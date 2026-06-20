//! Server-side runtime gateway routes.
//!
//! Wave 1-2 scope:
//! - encrypted BYOK/provider credential metadata
//! - runtime run/stage/provider ledgers
//! - transcript-only polish probe retained for latency testing
//! - WebSocket audio runtime MVP: client audio -> Deepgram -> server polish
//!
//! This module intentionally does not persist raw transcript/audio by default.

use std::{
    future,
    time::{Duration, Instant},
};

use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit},
};
use axum::{
    Json,
    extract::{Path, Query, State, WebSocketUpgrade, ws::Message},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use base64::{Engine as _, engine::general_purpose};
use futures_util::{SinkExt, StreamExt};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio_tungstenite::{WebSocketStream, tungstenite::Message as DgMessage};
use uuid::Uuid;

use crate::notification_hub::DesktopNotification;
use crate::stt::{self, runtime_stt_credential_provider};
use crate::voice_polish_standalone::{
    build_rewrite_system_prompt, build_rewrite_user_message, build_voice_system_prompt,
    build_voice_user_message,
};
use crate::{AppState, auth::AuthUser, memory_hygiene, org_quota, tenant};

const GROQ_ENDPOINT: &str = "https://api.groq.com/openai/v1/chat/completions";
const GROQ_MODEL_FAST: &str = "llama-3.1-8b-instant";
const GROQ_MODEL_SMART: &str = "meta-llama/llama-4-scout-17b-16e-instruct";
const OPENAI_AUDIO_TRANSCRIPTIONS_ENDPOINT: &str = "https://api.openai.com/v1/audio/transcriptions";
const DEFAULT_OPENAI_TRANSCRIBE_MODEL: &str = "whisper-1";
const DEFAULT_DEEPSEEK_MESSAGE_POLISH_MODEL: &str = "deepseek-v4-flash";
const DEEPGRAM_VALIDATE_ENDPOINT: &str = "https://api.deepgram.com/v1/projects";
const GROQ_VALIDATE_ENDPOINT: &str = "https://api.groq.com/openai/v1/models";
const OPENAI_VALIDATE_ENDPOINT: &str = "https://api.openai.com/v1/models";
const GEMINI_VALIDATE_ENDPOINT: &str = "https://generativelanguage.googleapis.com/v1beta/models";
const GATEWAY_VALIDATE_ENDPOINT: &str = "https://gateway.outreachdeal.com/v1/chat/completions";

fn selected_polish_model(_selected_model: &str) -> &'static str {
    GROQ_MODEL_SMART
}

fn learning_judge_model() -> String {
    match std::env::var("AIRNOTE_LEARNING_JUDGE_MODEL")
        .unwrap_or_else(|_| GROQ_MODEL_FAST.to_string())
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "" | "fast" | "8b" => GROQ_MODEL_FAST.to_string(),
        "smart" | "scout" => GROQ_MODEL_SMART.to_string(),
        other => other.to_string(),
    }
}

type DgSocket = WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
type DgSink = futures_util::stream::SplitSink<DgSocket, DgMessage>;
type DgStream = futures_util::stream::SplitStream<DgSocket>;

// ── Request / response models ───────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct MessagePolishRequest {
    pub text: String,
    #[serde(default)]
    pub client_run_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct MessagePolishResponse {
    pub run_id: String,
    pub output: String,
    pub model_used: String,
    pub prompt_version: String,
    pub latency_ms: RuntimeLatency,
}

#[derive(Debug, Deserialize)]
pub struct VoicePolishRequest {
    pub transcript: String,
    #[serde(default = "default_output_language")]
    pub output_language: String,
    #[serde(default = "default_selected_model")]
    pub selected_model: String,
    #[serde(default)]
    pub screen_context: Option<String>,
    #[serde(default)]
    pub safe_vocab_terms: Vec<String>,
    #[serde(default)]
    pub client_run_id: Option<String>,
    /// Optional per-request tone override (e.g. the iOS keyboard "rewrite selection"
    /// picks a tone per tap). When present it wins over the account's saved tone_preset;
    /// when absent — every existing caller — behavior is byte-for-byte unchanged.
    #[serde(default)]
    pub tone_preset: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct VoiceWavRequest {
    pub wav_b64: String,
    #[serde(default = "default_voice_wav_mode")]
    pub mode: String,
    #[serde(default = "default_output_language")]
    pub output_language: String,
    #[serde(default = "default_selected_model")]
    pub selected_model: String,
    #[serde(default)]
    pub screen_context: Option<String>,
    #[serde(default)]
    pub safe_vocab_terms: Vec<String>,
    #[serde(default)]
    pub client_run_id: Option<String>,
    #[serde(default)]
    pub recording_id: Option<String>,
    #[serde(default)]
    pub device_id: Option<String>,
    #[serde(default)]
    pub platform: Option<String>,
    #[serde(default)]
    pub app_version: Option<String>,
    #[serde(default)]
    pub stt_provider: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct VoicePolishResponse {
    pub run_id: String,
    pub output: String,
    pub model_used: String,
    pub prompt_version: String,
    pub latency_ms: RuntimeLatency,
}

#[derive(Debug, Serialize)]
pub struct RuntimeLatency {
    pub prompt: i64,
    pub model: i64,
    pub total: i64,
}

#[derive(Debug, Serialize)]
pub struct VoiceWavResponse {
    pub run_id: String,
    pub transcript: String,
    pub transcript_hash: String,
    pub output: String,
    pub model_used: String,
    pub prompt_version: String,
    pub latency_ms: RuntimeAudioLatency,
}

#[derive(Debug, Serialize)]
pub struct RuntimeAudioLatency {
    pub stt: i64,
    pub polish: i64,
    pub total: i64,
}

#[derive(Debug, Deserialize)]
pub struct SaveCredentialRequest {
    pub provider: String,
    pub secret: String,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub org_id: Option<Uuid>,
    #[serde(default)]
    pub display_name: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CredentialSummary {
    pub id: Uuid,
    pub provider: String,
    pub scope: String,
    pub org_id: Option<Uuid>,
    pub account_id: Option<Uuid>,
    pub display_name: String,
    pub secret_last4: String,
    pub status: String,
    pub validated_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_used_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_error: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Deserialize)]
pub struct DryRunRequest {
    #[serde(default)]
    pub client_run_id: Option<String>,
    #[serde(default = "default_runtime_mode")]
    pub mode: String,
    #[serde(default = "default_runtime_source")]
    pub source: String,
    #[serde(default)]
    pub device_id: Option<String>,
    #[serde(default)]
    pub platform: Option<String>,
    #[serde(default)]
    pub app_version: Option<String>,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Serialize)]
pub struct DryRunResponse {
    pub run_id: Uuid,
    pub status: String,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct RuntimeStatusResponse {
    pub credential_encryption_configured: bool,
    pub active_credential_count: i64,
    pub runtime_session_count: i64,
    pub learning_event_count: i64,
    pub personal_replacement_count: i64,
    pub personal_vocab_count: i64,
    pub personal_alias_count: i64,
    pub active_edit_policy_count: i64,
    pub server_memory_ready: bool,
}

#[derive(Debug, Deserialize)]
pub struct RuntimeWsQuery {
    pub token: String,
}

#[derive(Debug, Deserialize)]
pub struct ClientEventRequest {
    pub event_type: String,
    #[serde(default)]
    pub client_run_id: Option<String>,
    #[serde(default)]
    pub recording_id: Option<String>,
    #[serde(default)]
    pub run_id: Option<Uuid>,
    #[serde(default)]
    pub classification: Option<String>,
    #[serde(default)]
    pub input_hash: Option<String>,
    #[serde(default)]
    pub output_hash: Option<String>,
    #[serde(default)]
    pub corrected_hash: Option<String>,
    #[serde(default)]
    pub payload: Value,
    #[serde(default)]
    pub notification: Option<DesktopNotification>,
}

#[derive(Debug, Serialize)]
pub struct ClientEventResponse {
    pub stored: bool,
    pub notified: bool,
}

#[derive(Debug, Deserialize)]
pub struct RuntimeConfirmBatchRequest {
    #[serde(default)]
    pub recording_id: Option<String>,
    pub items: Vec<RuntimeConfirmBatchItem>,
}

#[derive(Debug, Deserialize)]
pub struct RuntimeConfirmBatchItem {
    pub original: String,
    pub corrected: String,
    #[serde(default)]
    pub term_type: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RuntimeConfirmBatchResponse {
    pub learned_count: usize,
    pub blocked_count: usize,
    pub learned_terms: Vec<String>,
    pub server_judgment: Value,
}

#[derive(Debug, Deserialize)]
pub struct AnalyzeEditLearningRequest {
    #[serde(default)]
    pub recording_id: Option<String>,
    pub transcript: String,
    pub ai_output: String,
    pub user_kept: String,
    #[serde(default)]
    pub candidates: Vec<LearningReviewCandidate>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct LearningReviewCandidate {
    pub original: String,
    pub corrected: String,
    pub term_type: String,
    pub learnable: bool,
    pub tag: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct UserEditSpan {
    pub pasted_span: String,
    pub kept_span: String,
    pub left_context: String,
    pub right_context: String,
    pub pasted_start: usize,
    pub kept_start: usize,
}

#[derive(Debug, Serialize)]
pub struct AnalyzeEditLearningResponse {
    pub candidates: Vec<LearningReviewCandidate>,
    pub changed: bool,
    pub source: &'static str,
}

#[derive(Debug, Deserialize)]
pub struct RuntimeRunsQuery {
    #[serde(default = "default_runs_limit")]
    pub limit: i64,
}

#[derive(Debug, Serialize)]
pub struct RuntimeRunSummary {
    pub id: Uuid,
    pub account_id: Uuid,
    pub account_email: String,
    pub client_run_id: Option<String>,
    pub mode: String,
    pub source: String,
    pub platform: Option<String>,
    pub app_version: Option<String>,
    pub status: String,
    pub error_kind: Option<String>,
    pub input_hash: Option<String>,
    pub output_hash: Option<String>,
    pub provider_summary: Value,
    pub latency_json: Value,
    pub metadata_json: Value,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize)]
pub struct RuntimeRunDetail {
    pub run: RuntimeRunSummary,
    pub stages: Vec<RuntimeStageSummary>,
    pub provider_usage: Vec<RuntimeProviderUsageSummary>,
}

#[derive(Debug, Serialize)]
pub struct RuntimeLearningEventSummary {
    pub id: Uuid,
    pub account_id: Uuid,
    pub account_email: String,
    pub run_id: Option<Uuid>,
    pub recording_id: Option<String>,
    pub event_type: String,
    pub classification: Option<String>,
    pub input_hash: Option<String>,
    pub output_hash: Option<String>,
    pub corrected_hash: Option<String>,
    pub payload_json: Value,
    pub server_judgment: Value,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize)]
pub struct RuntimeStageSummary {
    pub id: Uuid,
    pub stage: String,
    pub status: String,
    pub latency_ms: Option<i64>,
    pub error_kind: Option<String>,
    pub metadata_json: Value,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize)]
pub struct RuntimeProviderUsageSummary {
    pub id: Uuid,
    pub provider: String,
    pub model: Option<String>,
    pub credential_scope: String,
    pub request_ms: Option<i64>,
    pub ttft_ms: Option<i64>,
    pub stream_ms: Option<i64>,
    pub total_ms: Option<i64>,
    pub timeout_ms: Option<i64>,
    pub status: String,
    pub error_kind: Option<String>,
    pub fallback_reason: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

fn default_runs_limit() -> i64 {
    50
}

// ── Credential endpoints ────────────────────────────────────────────────────

pub async fn save_credential(
    State(state): State<AppState>,
    user: AuthUser,
    Json(req): Json<SaveCredentialRequest>,
) -> Result<Json<CredentialSummary>, (StatusCode, Json<Value>)> {
    let provider = normalize_provider(&req.provider)?;
    let scope = normalize_scope(req.scope.as_deref())?;
    let secret = req.secret.trim();
    if secret.len() < 8 {
        return Err(json_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "provider secret must be at least 8 characters",
        ));
    }
    if scope == "org" && req.org_id.is_none() {
        return Err(json_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "org_id is required for org-scoped credentials",
        ));
    }
    if let Some(org_id) = req.org_id {
        tenant::ensure_org_member(&state, user.account_id, org_id).await?;
    }

    validate_provider_secret(&provider, secret)
        .await
        .map_err(ProviderValidationError::into_response)?;

    let encrypted = encrypt_secret(&state, secret)?;
    let display_name = req
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(&provider)
        .chars()
        .take(80)
        .collect::<String>();
    let secret_last4 = last4(secret);

    let account_id = if scope == "user" {
        Some(user.account_id)
    } else {
        None
    };
    let org_id = if scope == "org" { req.org_id } else { None };

    let row = if scope == "user" {
        sqlx::query_as::<_, CredentialRow>(
            "UPDATE runtime_provider_credentials
                SET display_name = $3,
                    secret_ciphertext = $4,
                    secret_nonce = $5,
                    secret_key_version = 'v1',
                    secret_last4 = $6,
                    status = 'active',
                    validated_at = now(),
                    last_error = NULL,
                    updated_at = now()
              WHERE account_id = $1
                AND provider = $2
                AND scope = 'user'
                AND status <> 'revoked'
              RETURNING id, provider, scope, org_id, account_id, display_name, secret_last4,
                        status, validated_at, last_used_at, last_error, created_at, updated_at",
        )
        .bind(user.account_id)
        .bind(&provider)
        .bind(&display_name)
        .bind(&encrypted.ciphertext)
        .bind(&encrypted.nonce)
        .bind(&secret_last4)
        .fetch_optional(&state.db)
        .await
        .map_err(db_err)?
    } else if scope == "org" {
        sqlx::query_as::<_, CredentialRow>(
            "UPDATE runtime_provider_credentials
                SET display_name = $3,
                    secret_ciphertext = $4,
                    secret_nonce = $5,
                    secret_key_version = 'v1',
                    secret_last4 = $6,
                    status = 'active',
                    validated_at = now(),
                    last_error = NULL,
                    updated_at = now()
              WHERE org_id = $1
                AND provider = $2
                AND scope = 'org'
                AND status <> 'revoked'
              RETURNING id, provider, scope, org_id, account_id, display_name, secret_last4,
                        status, validated_at, last_used_at, last_error, created_at, updated_at",
        )
        .bind(org_id)
        .bind(&provider)
        .bind(&display_name)
        .bind(&encrypted.ciphertext)
        .bind(&encrypted.nonce)
        .bind(&secret_last4)
        .fetch_optional(&state.db)
        .await
        .map_err(db_err)?
    } else {
        None
    };

    let row = if let Some(row) = row {
        row
    } else {
        sqlx::query_as::<_, CredentialRow>(
            "INSERT INTO runtime_provider_credentials
                (org_id, account_id, scope, provider, display_name, secret_ciphertext,
                 secret_nonce, secret_key_version, secret_last4, status, validated_at, created_by)
             VALUES ($1, $2, $3, $4, $5, $6, $7, 'v1', $8, 'active', now(), $9)
             RETURNING id, provider, scope, org_id, account_id, display_name, secret_last4,
                       status, validated_at, last_used_at, last_error, created_at, updated_at",
        )
        .bind(org_id)
        .bind(account_id)
        .bind(&scope)
        .bind(&provider)
        .bind(&display_name)
        .bind(&encrypted.ciphertext)
        .bind(&encrypted.nonce)
        .bind(&secret_last4)
        .bind(user.account_id)
        .fetch_one(&state.db)
        .await
        .map_err(db_err)?
    };

    tracing::info!(
        "[runtime] credential saved account={} scope={} provider={} credential={}",
        user.account_id,
        scope,
        provider,
        row.id
    );

    Ok(Json(row.into()))
}

pub async fn list_credentials(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<CredentialSummary>>, (StatusCode, Json<Value>)> {
    let rows = sqlx::query_as::<_, CredentialRow>(
        "SELECT id, provider, scope, org_id, account_id, display_name, secret_last4,
                status, validated_at, last_used_at, last_error, created_at, updated_at
           FROM runtime_provider_credentials
          WHERE status <> 'revoked'
            AND (
                account_id = $1
                OR org_id IN (SELECT org_id FROM org_members WHERE account_id = $1)
                OR scope = 'airnote_managed'
            )
          ORDER BY updated_at DESC",
    )
    .bind(user.account_id)
    .fetch_all(&state.db)
    .await
    .map_err(db_err)?;

    Ok(Json(rows.into_iter().map(Into::into).collect()))
}

pub async fn validate_credential(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<CredentialSummary>, (StatusCode, Json<Value>)> {
    let row = load_owned_credential_secret(&state, user.account_id, id).await?;
    let secret = decrypt_secret(&state, &row.secret_ciphertext, &row.secret_nonce)?;
    if secret.trim().is_empty() {
        return Err(json_error(
            StatusCode::BAD_REQUEST,
            "credential secret is empty",
        ));
    }

    if let Err(err) = validate_provider_secret(&row.provider, secret.trim()).await {
        let status = if err.permanent {
            "invalid"
        } else {
            "validation_failed"
        };
        sqlx::query(
            "UPDATE runtime_provider_credentials
                SET status = $2, validated_at = now(), last_error = $3, updated_at = now()
              WHERE id = $1",
        )
        .bind(id)
        .bind(status)
        .bind(&err.message)
        .execute(&state.db)
        .await
        .map_err(db_err)?;
        return Err(err.into_response());
    }

    let row = sqlx::query_as::<_, CredentialRow>(
        "UPDATE runtime_provider_credentials
            SET status = 'active', validated_at = now(), last_error = NULL, updated_at = now()
          WHERE id = $1
          RETURNING id, provider, scope, org_id, account_id, display_name, secret_last4,
                    status, validated_at, last_used_at, last_error, created_at, updated_at",
    )
    .bind(id)
    .fetch_one(&state.db)
    .await
    .map_err(db_err)?;

    Ok(Json(row.into()))
}

pub async fn revoke_credential(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, (StatusCode, Json<Value>)> {
    let row = load_owned_credential_secret(&state, user.account_id, id).await?;
    if row.account_id != Some(user.account_id) && row.org_id.is_some() {
        tenant::ensure_org_member(&state, user.account_id, row.org_id.unwrap()).await?;
    }

    sqlx::query(
        "UPDATE runtime_provider_credentials
            SET status = 'revoked', updated_at = now()
          WHERE id = $1",
    )
    .bind(id)
    .execute(&state.db)
    .await
    .map_err(db_err)?;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn status(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<RuntimeStatusResponse>, (StatusCode, Json<Value>)> {
    let active_credential_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::BIGINT
           FROM runtime_provider_credentials
          WHERE status = 'active'
            AND (
                account_id = $1
                OR org_id IN (SELECT org_id FROM org_members WHERE account_id = $1)
                OR scope = 'airnote_managed'
            )",
    )
    .bind(user.account_id)
    .fetch_one(&state.db)
    .await
    .map_err(db_err)?;

    let runtime_session_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*)::BIGINT FROM runtime_sessions WHERE account_id = $1")
            .bind(user.account_id)
            .fetch_one(&state.db)
            .await
            .map_err(db_err)?;

    let learning_event_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::BIGINT FROM runtime_learning_events WHERE account_id = $1",
    )
    .bind(user.account_id)
    .fetch_one(&state.db)
    .await
    .map_err(db_err)?;

    let personal_replacement_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::BIGINT
           FROM personal_stt_replacements
          WHERE account_id = $1 AND status = 'active'",
    )
    .bind(user.account_id)
    .fetch_one(&state.db)
    .await
    .map_err(db_err)?;

    let personal_vocab_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::BIGINT FROM personal_vocab_terms WHERE account_id = $1 AND status = 'active'",
    )
    .bind(user.account_id)
    .fetch_one(&state.db)
    .await
    .map_err(db_err)?;

    let personal_alias_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::BIGINT FROM personal_stt_replacements WHERE account_id = $1 AND status = 'active'",
    )
    .bind(user.account_id)
    .fetch_one(&state.db)
    .await
    .map_err(db_err)?;

    let active_edit_policy_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::BIGINT FROM personal_edit_policy_rules WHERE account_id = $1 AND status = 'active'",
    )
    .bind(user.account_id)
    .fetch_one(&state.db)
    .await
    .map_err(db_err)?;

    Ok(Json(RuntimeStatusResponse {
        credential_encryption_configured: !state.runtime_credentials_key.trim().is_empty(),
        active_credential_count,
        runtime_session_count,
        learning_event_count,
        personal_replacement_count,
        personal_vocab_count,
        personal_alias_count,
        active_edit_policy_count,
        server_memory_ready: personal_vocab_count + personal_alias_count + active_edit_policy_count
            > 0,
    }))
}

pub async fn list_runs(
    State(state): State<AppState>,
    user: AuthUser,
    Query(query): Query<RuntimeRunsQuery>,
) -> Result<Json<Vec<RuntimeRunSummary>>, (StatusCode, Json<Value>)> {
    let limit = query.limit.clamp(1, 200);
    let rows = sqlx::query_as::<_, RuntimeRunRow>(
        "SELECT id, client_run_id, mode, source, platform, app_version, status,
                error_kind, input_hash, output_hash, provider_summary, latency_json,
                metadata_json, created_at, updated_at, account_id, account_email
           FROM (
              SELECT rs.id, rs.client_run_id, rs.mode, rs.source, rs.platform, rs.app_version,
                     rs.status, rs.error_kind, rs.input_hash, rs.output_hash,
                     rs.provider_summary, rs.latency_json, rs.metadata_json,
                     rs.created_at, rs.updated_at, rs.account_id, a.email AS account_email
                FROM runtime_sessions rs
                JOIN accounts a ON a.id = rs.account_id
               WHERE rs.account_id = $1
                  OR rs.org_id IN (SELECT org_id FROM org_members WHERE account_id = $1)
           ) visible_runs
          ORDER BY created_at DESC
          LIMIT $2",
    )
    .bind(user.account_id)
    .bind(limit)
    .fetch_all(&state.db)
    .await
    .map_err(db_err)?;

    Ok(Json(rows.into_iter().map(Into::into).collect()))
}

pub async fn run_detail(
    State(state): State<AppState>,
    user: AuthUser,
    Path(run_id): Path<Uuid>,
) -> Result<Json<RuntimeRunDetail>, (StatusCode, Json<Value>)> {
    let run = sqlx::query_as::<_, RuntimeRunRow>(
        "SELECT id, client_run_id, mode, source, platform, app_version, status,
                error_kind, input_hash, output_hash, provider_summary, latency_json,
                metadata_json, created_at, updated_at, account_id, account_email
           FROM (
              SELECT rs.id, rs.client_run_id, rs.mode, rs.source, rs.platform, rs.app_version,
                     rs.status, rs.error_kind, rs.input_hash, rs.output_hash,
                     rs.provider_summary, rs.latency_json, rs.metadata_json,
                     rs.created_at, rs.updated_at, rs.account_id, a.email AS account_email
                FROM runtime_sessions rs
                JOIN accounts a ON a.id = rs.account_id
               WHERE rs.id = $1
                 AND (
                    rs.account_id = $2
                    OR rs.org_id IN (SELECT org_id FROM org_members WHERE account_id = $2)
                 )
           ) visible_run",
    )
    .bind(run_id)
    .bind(user.account_id)
    .fetch_optional(&state.db)
    .await
    .map_err(db_err)?
    .ok_or_else(|| json_error(StatusCode::NOT_FOUND, "runtime run not found"))?;

    let stages = sqlx::query_as::<_, RuntimeStageRow>(
        "SELECT id, stage, status, latency_ms, error_kind, metadata_json, created_at
           FROM runtime_stage_events
          WHERE run_id = $1
          ORDER BY created_at ASC",
    )
    .bind(run_id)
    .fetch_all(&state.db)
    .await
    .map_err(db_err)?;

    let provider_usage = sqlx::query_as::<_, RuntimeProviderUsageRow>(
        "SELECT id, provider, model, credential_scope, request_ms, ttft_ms, stream_ms,
                total_ms, timeout_ms, status, error_kind, fallback_reason, created_at
           FROM runtime_provider_usage
          WHERE run_id = $1
          ORDER BY attempt_index ASC, created_at ASC",
    )
    .bind(run_id)
    .fetch_all(&state.db)
    .await
    .map_err(db_err)?;

    Ok(Json(RuntimeRunDetail {
        run: run.into(),
        stages: stages.into_iter().map(Into::into).collect(),
        provider_usage: provider_usage.into_iter().map(Into::into).collect(),
    }))
}

pub async fn list_learning_events(
    State(state): State<AppState>,
    user: AuthUser,
    Query(query): Query<RuntimeRunsQuery>,
) -> Result<Json<Vec<RuntimeLearningEventSummary>>, (StatusCode, Json<Value>)> {
    let limit = query.limit.clamp(1, 200);
    let rows = sqlx::query_as::<_, RuntimeLearningEventRow>(
        "SELECT id, account_id, account_email, run_id, recording_id, event_type,
                classification, input_hash, output_hash, corrected_hash,
                payload_json, server_judgment, created_at
           FROM (
              SELECT e.id, e.account_id, a.email AS account_email, e.run_id, e.recording_id,
                     e.event_type, e.classification, e.input_hash, e.output_hash,
                     e.corrected_hash, e.payload_json, e.server_judgment, e.created_at
                FROM runtime_learning_events e
                JOIN accounts a ON a.id = e.account_id
               WHERE e.account_id = $1
                  OR e.org_id IN (SELECT org_id FROM org_members WHERE account_id = $1)
           ) visible_events
          ORDER BY created_at DESC
          LIMIT $2",
    )
    .bind(user.account_id)
    .bind(limit)
    .fetch_all(&state.db)
    .await
    .map_err(db_err)?;

    Ok(Json(rows.into_iter().map(Into::into).collect()))
}

pub async fn analyze_edit_learning(
    State(state): State<AppState>,
    user: AuthUser,
    Json(req): Json<AnalyzeEditLearningRequest>,
) -> Result<Json<AnalyzeEditLearningResponse>, (StatusCode, Json<Value>)> {
    let edit_spans = extract_user_edit_spans(&req.ai_output, &req.user_kept);
    let generated_candidates = if req.candidates.is_empty() {
        let mut candidates = deterministic_user_edit_span_candidates_for_request(&req, &edit_spans);
        merge_learning_candidates(&mut candidates, deterministic_learning_candidates(&req));
        let llm_candidates = learning_judge_candidates(&state, user.account_id, &req, &edit_spans)
            .await
            .unwrap_or_default();
        merge_learning_candidates(&mut candidates, llm_candidates);
        candidates
    } else {
        let mut candidates = deterministic_user_edit_span_candidates_for_request(&req, &edit_spans);
        let validated = validate_learning_candidates_with_judge(&state, user.account_id, &req)
            .await
            .unwrap_or_default();
        merge_learning_candidates(&mut candidates, validated);
        candidates
    };

    let effective_req = if generated_candidates.is_empty() {
        req
    } else {
        AnalyzeEditLearningRequest {
            candidates: generated_candidates,
            ..req
        }
    };
    let refined = refine_learning_review_candidates(&effective_req);
    let changed = refined != effective_req.candidates;
    Ok(Json(AnalyzeEditLearningResponse {
        candidates: refined,
        changed,
        source: if effective_req.candidates.is_empty() {
            "server_deterministic_alignment"
        } else {
            "server_llm_learning_judge"
        },
    }))
}

pub async fn confirm_learning_batch(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Json(req): Json<RuntimeConfirmBatchRequest>,
) -> Result<Json<RuntimeConfirmBatchResponse>, (StatusCode, Json<Value>)> {
    if req.items.is_empty() {
        return Err(json_error(StatusCode::BAD_REQUEST, "items are required"));
    }

    let tenant_ctx = tenant::resolve_tenant(&state, &user, &headers).await?;
    let org_id = tenant_ctx.active_org_id;
    let mut accepted_terms = Vec::new();
    let mut accepted_aliases = Vec::new();
    let mut learned_terms = Vec::new();
    let mut original_hash_parts = Vec::new();

    for item in &req.items {
        let corrected = item.corrected.trim();
        let original = item.original.trim();
        if corrected.is_empty() {
            continue;
        }
        let term_type = item
            .term_type
            .as_deref()
            .map(str::trim)
            .filter(|term_type| is_allowed_term_type(term_type))
            .unwrap_or_else(|| infer_term_type_from_target(corrected));

        accepted_terms.push(json!({
            "term": corrected,
            "term_type": term_type,
            "weight": 1.0,
            "source": "server_confirm_batch",
        }));
        if !original.is_empty() {
            accepted_aliases.push(json!({
                "transcript_form": original,
                "correct_form": corrected,
                "edit_type": "replace",
                "term_type": term_type,
                "source": "server_confirm_batch",
            }));
            original_hash_parts.push(original.to_string());
        }
        if !learned_terms.iter().any(|term| term == corrected) {
            learned_terms.push(corrected.to_string());
        }
    }

    if accepted_terms.is_empty() && accepted_aliases.is_empty() {
        return Err(json_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "no learnable confirm items",
        ));
    }

    let payload = json!({
        "learned": true,
        "notify": true,
        "source": "server_confirm_batch",
        "promoted_count": req.items.len(),
        "promoted_term_count": learned_terms.len(),
        "capture_method": "user_confirmed_modal",
        "memory": {
            "accepted_terms": accepted_terms,
            "accepted_aliases": accepted_aliases,
        },
    });
    let event_req = ClientEventRequest {
        event_type: "classify_edit_result".to_string(),
        client_run_id: None,
        recording_id: req.recording_id.clone(),
        run_id: None,
        classification: Some("STT_ERROR".to_string()),
        input_hash: Some(content_hash(&original_hash_parts.join("|"))),
        output_hash: None,
        corrected_hash: Some(content_hash(&learned_terms.join("|"))),
        payload,
        notification: None,
    };

    let server_judgment =
        judge_and_upsert_client_learning_event(&state, &user, org_id, None, &event_req)
            .await
            .map_err(db_err)?;

    sqlx::query(
        "INSERT INTO runtime_learning_events
            (account_id, org_id, recording_id, event_type, classification,
             input_hash, corrected_hash, payload_json, server_judgment)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(user.account_id)
    .bind(org_id)
    .bind(event_req.recording_id.as_deref())
    .bind(event_req.event_type.as_str())
    .bind(event_req.classification.as_deref())
    .bind(event_req.input_hash.as_deref())
    .bind(event_req.corrected_hash.as_deref())
    .bind(&event_req.payload)
    .bind(&server_judgment)
    .execute(&state.db)
    .await
    .map_err(db_err)?;

    let accepted_aliases = server_judgment
        .get("accepted_aliases")
        .and_then(Value::as_i64)
        .unwrap_or(0)
        .max(0) as usize;
    let accepted_terms = server_judgment
        .get("accepted_terms")
        .and_then(Value::as_i64)
        .unwrap_or(0)
        .max(0) as usize;
    let learned_count = accepted_aliases.max(accepted_terms);
    let blocked_count = req.items.len().saturating_sub(learned_count);

    // Emit UI-native notification based on actual server judgment result.
    if learned_count > 0 {
        let term_label = if learned_terms.len() == 1 {
            learned_terms[0].clone()
        } else {
            format!("{} corrections", learned_count)
        };
        let message = format!(
            "Saved {} correction{}",
            learned_count,
            if learned_count == 1 { "" } else { "s" }
        );
        tracing::info!(
            "[runtime] notify vocab-learned account={} learned={} blocked={}",
            user.account_id,
            learned_count,
            blocked_count
        );
        state
            .notifications
            .emit(
                user.account_id,
                DesktopNotification {
                    kind: "vocab-learned".to_string(),
                    payload: json!({
                        "term": term_label,
                        "message": message,
                    }),
                },
            )
            .await;
    } else {
        tracing::info!(
            "[runtime] no notification — all {} item(s) blocked account={}",
            req.items.len(),
            user.account_id
        );
    }

    Ok(Json(RuntimeConfirmBatchResponse {
        learned_count,
        blocked_count,
        learned_terms,
        server_judgment,
    }))
}

pub async fn client_event(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Json(req): Json<ClientEventRequest>,
) -> Result<Json<ClientEventResponse>, (StatusCode, Json<Value>)> {
    let event_type = req.event_type.trim();
    if event_type.is_empty() {
        return Err(json_error(
            StatusCode::BAD_REQUEST,
            "event_type is required",
        ));
    }

    let tenant_ctx = tenant::resolve_tenant(&state, &user, &headers).await?;
    let org_id = tenant_ctx.active_org_id;
    let run_id = if let Some(run_id) = req.run_id {
        let owned: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM runtime_sessions WHERE id = $1 AND account_id = $2
            )",
        )
        .bind(run_id)
        .bind(user.account_id)
        .fetch_one(&state.db)
        .await
        .map_err(db_err)?;
        if owned { Some(run_id) } else { None }
    } else if let Some(client_run_id) = req.client_run_id.as_deref() {
        sqlx::query_scalar(
            "SELECT id
               FROM runtime_sessions
              WHERE account_id = $1 AND client_run_id = $2
              ORDER BY created_at DESC
              LIMIT 1",
        )
        .bind(user.account_id)
        .bind(client_run_id)
        .fetch_optional(&state.db)
        .await
        .map_err(db_err)?
    } else {
        None
    };

    let server_judgment =
        judge_and_upsert_client_learning_event(&state, &user, org_id, run_id, &req)
            .await
            .map_err(db_err)?;

    sqlx::query(
        "INSERT INTO runtime_learning_events
            (account_id, org_id, run_id, recording_id, event_type, classification,
             input_hash, output_hash, corrected_hash, payload_json, server_judgment)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
    )
    .bind(user.account_id)
    .bind(org_id)
    .bind(run_id)
    .bind(req.recording_id.as_deref())
    .bind(event_type)
    .bind(req.classification.as_deref())
    .bind(req.input_hash.as_deref())
    .bind(req.output_hash.as_deref())
    .bind(req.corrected_hash.as_deref())
    .bind(&req.payload)
    .bind(&server_judgment)
    .execute(&state.db)
    .await
    .map_err(db_err)?;

    let notified = if let Some(notification) = req.notification {
        state
            .notifications
            .emit(user.account_id, notification)
            .await;
        true
    } else {
        false
    };

    Ok(Json(ClientEventResponse {
        stored: true,
        notified,
    }))
}

pub async fn notifications_ws(
    State(state): State<AppState>,
    Query(query): Query<RuntimeWsQuery>,
    ws: WebSocketUpgrade,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let (account_id, email, _) = crate::auth::resolve_ws_token(&query.token, &state)
        .await
        .ok_or_else(|| (StatusCode::UNAUTHORIZED, "invalid or expired token".into()))?;

    Ok(ws.on_upgrade(move |socket| async move {
        handle_notifications_ws(state, account_id, email, socket).await;
    }))
}

async fn handle_notifications_ws(
    state: AppState,
    account_id: Uuid,
    email: String,
    socket: axum::extract::ws::WebSocket,
) {
    let mut rx = state.notifications.join(account_id).await;
    let (mut sink, mut stream) = socket.split();

    let _ = sink
        .send(Message::Text(
            json!({
                "type": "notification.connected",
                "version": 1,
                "account_id": account_id,
                "email": email,
            })
            .to_string(),
        ))
        .await;

    loop {
        tokio::select! {
            outbound = rx.recv() => {
                let Some(notification) = outbound else { break };
                let Ok(text) = serde_json::to_string(&notification) else { continue };
                if sink.send(Message::Text(text)).await.is_err() {
                    break;
                }
            }
            inbound = stream.next() => {
                let Some(Ok(msg)) = inbound else { break };
                match msg {
                    Message::Text(text) => {
                        if serde_json::from_str::<Value>(&text)
                            .ok()
                            .and_then(|v| v.get("type").and_then(Value::as_str).map(str::to_string))
                            .as_deref()
                            == Some("ping")
                        {
                            let _ = sink.send(Message::Text(json!({"type": "pong", "version": 1}).to_string())).await;
                        }
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
        }
    }
}

// ── Dry run / WS skeleton ───────────────────────────────────────────────────

pub async fn voice_dry_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Json(req): Json<DryRunRequest>,
) -> Result<Json<DryRunResponse>, (StatusCode, Json<Value>)> {
    let tenant_ctx = tenant::resolve_tenant(&state, &user, &headers).await?;
    let run_id = create_runtime_session(
        &state,
        user.account_id,
        tenant_ctx.active_org_id,
        req.client_run_id.as_deref(),
        &req.mode,
        &req.source,
        req.device_id.as_deref(),
        req.platform.as_deref(),
        req.app_version.as_deref(),
        req.metadata,
    )
    .await?;
    insert_stage_event(
        &state,
        run_id,
        "dry_run",
        "ok",
        Some(0),
        None,
        json!({"message": "server runtime dry-run accepted"}),
    )
    .await?;
    mark_runtime_session(&state, run_id, "completed", None).await?;

    Ok(Json(DryRunResponse {
        run_id,
        status: "completed".to_string(),
        message: "server runtime dry-run accepted".to_string(),
    }))
}

pub async fn voice_ws(
    State(state): State<AppState>,
    Query(query): Query<RuntimeWsQuery>,
    ws: WebSocketUpgrade,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let (account_id, email, _) = crate::auth::resolve_ws_token(&query.token, &state)
        .await
        .ok_or_else(|| (StatusCode::UNAUTHORIZED, "invalid or expired token".into()))?;

    Ok(ws.on_upgrade(move |socket| async move {
        handle_voice_ws(state, account_id, email, socket).await;
    }))
}

async fn handle_voice_ws(
    state: AppState,
    account_id: Uuid,
    email: String,
    socket: axum::extract::ws::WebSocket,
) {
    let (mut sink, mut stream) = socket.split();
    let server_stt_default = state.stt_provider.clone();
    let mut stt_provider = server_stt_default.clone();
    let welcome = json!({
        "type": "runtime.connected",
        "version": 1,
        "account_id": account_id,
        "email": email,
        "stt_provider": stt_provider,
        "audio_runtime": "deepgram_mvp"
    });
    if sink.send(Message::Text(welcome.to_string())).await.is_err() {
        return;
    }

    let mut active_run: Option<Uuid> = None;
    let mut dg_sink: Option<DgSink> = None;
    let mut dg_stream: Option<DgStream> = None;
    let mut audio_frames: i64 = 0;
    let mut audio_bytes: i64 = 0;
    let mut transcript_segments: Vec<String> = Vec::new();
    let mut latest_partial = String::new();
    let mut stt_started_at: Option<Instant> = None;
    let mut selected_model = default_selected_model();
    let mut output_language = default_output_language();
    let mut safe_vocab_terms: Vec<String> = Vec::new();
    let mut screen_context: Option<String> = None;
    let mut client_run_id: Option<String> = None;
    let mut saw_first_transcript_event = false;
    let mut stt_sample_rate: u32 = 16_000;

    loop {
        tokio::select! {
            browser_msg = stream.next() => {
                let Some(browser_msg) = browser_msg else { break };
                let Ok(msg) = browser_msg else { break };
                match msg {
                    Message::Text(text) => {
                let value = serde_json::from_str::<Value>(&text).unwrap_or_else(|_| json!({}));
                let msg_type = value.get("type").and_then(Value::as_str).unwrap_or("");
                match msg_type {
                    "voice.start" => {
                        if dg_sink.is_some() || active_run.is_some() {
                            let _ = sink.send(Message::Text(runtime_error_payload(
                                None,
                                client_run_id.as_deref(),
                                "recording_already_active",
                                None,
                                Some("a recording is already active on this websocket".to_string()),
                            ).to_string())).await;
                            continue;
                        }
                        client_run_id = value
                            .get("run_id")
                            .and_then(Value::as_str)
                            .map(str::to_string);
                        let mode = value
                            .get("mode")
                            .and_then(Value::as_str)
                            .unwrap_or("normal_voice");
                        selected_model = value
                            .get("selected_model")
                            .and_then(Value::as_str)
                            .unwrap_or("fast")
                            .to_string();
                        output_language = value
                            .get("output_language")
                            .and_then(Value::as_str)
                            .unwrap_or("hinglish")
                            .to_string();
                        screen_context = value
                            .get("screen_context")
                            .and_then(Value::as_str)
                            .map(|s| s.chars().take(500).collect::<String>());
                        safe_vocab_terms = value
                            .get("safe_vocab_terms")
                            .and_then(Value::as_array)
                            .map(|items| {
                                items
                                    .iter()
                                    .filter_map(Value::as_str)
                                    .map(|s| s.trim().to_string())
                                    .filter(|s| !s.is_empty())
                                    .take(30)
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default();
                        let sample_rate = value
                            .get("audio")
                            .and_then(|a| a.get("sample_rate"))
                            .and_then(Value::as_u64)
                            .unwrap_or(16_000)
                            .clamp(8_000, 48_000) as u32;
                        stt_provider = value
                            .get("stt_provider")
                            .and_then(Value::as_str)
                            .map(said_core::stt::resolve_provider_from_pref)
                            .unwrap_or_else(|| server_stt_default.clone());

                        let ws_org_id = match tenant::resolve_ws_org_id(&state, account_id).await {
                            Ok(org_id) => org_id,
                            Err((status, body)) => {
                                let _ = sink.send(Message::Text(runtime_error_payload(
                                    None,
                                    client_run_id.as_deref(),
                                    "org_resolution_failed",
                                    Some(status),
                                    Some(runtime_error_message(&body)),
                                ).to_string())).await;
                                continue;
                            }
                        };
                        if let Some(org_id) = ws_org_id {
                            if let Err((status, body)) =
                                org_quota::check_runtime_quota(&state, org_id).await
                            {
                                let _ = sink.send(Message::Text(runtime_error_payload(
                                    None,
                                    client_run_id.as_deref(),
                                    "quota_exceeded",
                                    Some(status),
                                    Some(runtime_error_message(&body)),
                                ).to_string())).await;
                                continue;
                            }
                        }

                        let run_result = create_runtime_session(
                            &state,
                            account_id,
                            ws_org_id,
                            client_run_id.as_deref(),
                            mode,
                            "desktop_voice",
                            value.get("device_id").and_then(Value::as_str),
                            value.get("platform").and_then(Value::as_str),
                            value.get("app_version").and_then(Value::as_str),
                            runtime_ws_start_metadata(
                                &value,
                                sample_rate,
                                &selected_model,
                                &output_language,
                                safe_vocab_terms.len(),
                                screen_context.as_deref(),
                            ),
                        )
                        .await;
                        let run_id = match run_result {
                            Ok(run_id) => run_id,
                            Err((status, body)) => {
                                let _ = sink.send(Message::Text(runtime_error_payload(
                                    None,
                                    client_run_id.as_deref(),
                                    "runtime_session_create_failed",
                                    Some(status),
                                    Some(runtime_error_message(&body)),
                                ).to_string())).await;
                                continue;
                            }
                        };
                        stt_sample_rate = sample_rate;
                        let credential_provider =
                            runtime_stt_credential_provider(&stt_provider);
                        let stt_credential = match runtime_provider_secret(
                            &state,
                            account_id,
                            ws_org_id,
                            credential_provider,
                        )
                        .await
                        {
                            Ok(secret) => secret,
                            Err((status, body)) => {
                                let err_kind = format!("{credential_provider}_credential_missing");
                                let _ = mark_runtime_session(
                                    &state,
                                    run_id,
                                    "failed",
                                    Some(&err_kind),
                                )
                                .await;
                                let _ = sink.send(Message::Text(runtime_error_payload(
                                    Some(run_id),
                                    client_run_id.as_deref(),
                                    &err_kind,
                                    Some(status),
                                    Some(runtime_error_message(&body)),
                                ).to_string())).await;
                                continue;
                            }
                        };
                        tracing::info!(
                            "[runtime] ws voice.start account={} run_id={} client_run_id={:?} stt={} sample_rate={} model={}",
                            account_id,
                            run_id,
                            client_run_id,
                            stt_provider,
                            sample_rate,
                            selected_model
                        );
                        let connect_start = Instant::now();
                        let stt_model = "nova-3";
                        match stt::connect_runtime_ws(
                            &stt_provider,
                            &stt_credential.secret,
                            sample_rate,
                        )
                        .await
                        {
                            Ok(socket) => {
                                let connect_ms = connect_start.elapsed().as_millis() as i64;
                                let (new_sink, new_stream) = socket.split();
                                dg_sink = Some(new_sink);
                                dg_stream = Some(new_stream);
                                active_run = Some(run_id);
                                audio_frames = 0;
                                audio_bytes = 0;
                                transcript_segments.clear();
                                latest_partial.clear();
                                stt_started_at = Some(Instant::now());
                                saw_first_transcript_event = false;
                                let _ =
                                    update_credential_used(&state, stt_credential.credential_id).await;
                                let _ = insert_provider_usage(
                                    &state,
                                    run_id,
                                    &stt_credential,
                                    credential_provider,
                                    Some(stt_model),
                                    Some(connect_ms),
                                    "connected",
                                    None,
                                ).await;
                                let _ = insert_stage_event(
                                    &state,
                                    run_id,
                                    "stt_ws_connected",
                                    "ok",
                                    Some(connect_ms),
                                    None,
                                    json!({
                                        "provider": credential_provider,
                                        "sample_rate": sample_rate,
                                        "credential_scope": stt_credential.scope
                                    }),
                                )
                                .await;
                                let _ = sink.send(Message::Text(json!({
                                    "type": "runtime.status",
                                    "version": 1,
                                    "run_id": run_id,
                                    "client_run_id": client_run_id.as_deref(),
                                    "phase": "stt_connected"
                                }).to_string())).await;
                            }
                            Err(e) => {
                                let connect_ms = connect_start.elapsed().as_millis() as i64;
                                let err_msg = e.to_string();
                                let connect_err = format!("{credential_provider}_connect_failed");
                                let _ = insert_provider_usage(
                                    &state,
                                    run_id,
                                    &stt_credential,
                                    credential_provider,
                                    Some(stt_model),
                                    Some(connect_ms),
                                    "error",
                                    Some(&connect_err),
                                ).await;
                                let _ = insert_stage_event(
                                    &state,
                                    run_id,
                                    "stt_ws_connect",
                                    "error",
                                    Some(connect_ms),
                                    Some(&connect_err),
                                    json!({"error": err_msg.chars().take(240).collect::<String>()}),
                                ).await;
                                let _ =
                                    mark_runtime_session(&state, run_id, "failed", Some(&connect_err))
                                        .await;
                                let _ = sink.send(Message::Text(runtime_error_payload(
                                    Some(run_id),
                                    client_run_id.as_deref(),
                                    &connect_err,
                                    None,
                                    Some("failed to connect to Deepgram".to_string()),
                                ).to_string())).await;
                            }
                        }
                    }
                    "audio.end" => {
                        if let Some(run_id) = active_run.take() {
                            if let Some(mut dg) = dg_sink.take() {
                                let _ = dg.send(DgMessage::Text(
                                    json!({"type": "CloseStream"}).to_string(),
                                ))
                                .await;
                            }
                            if let Some(mut dg) = dg_stream.take() {
                                drain_deepgram_finals(
                                    &mut dg,
                                    &mut transcript_segments,
                                    &mut latest_partial,
                                    std::time::Duration::from_millis(1800),
                                )
                                .await;
                            }
                            let _ = insert_stage_event(
                                &state,
                                run_id,
                                "audio_frames_received",
                                "ok",
                                None,
                                None,
                                json!({"frame_count": audio_frames, "audio_bytes": audio_bytes}),
                            )
                            .await;
                            let transcript = final_transcript(&transcript_segments, &latest_partial);
                            if transcript.trim().is_empty() {
                                let _ = mark_runtime_session(&state, run_id, "failed", Some("empty_transcript")).await;
                                let _ = sink.send(Message::Text(runtime_error_payload(
                                    Some(run_id),
                                    client_run_id.as_deref(),
                                    "empty_transcript",
                                    None,
                                    Some("server runtime did not receive any transcript from STT".to_string()),
                                ).to_string())).await;
                                continue;
                            }
                            let stt_ms = stt_started_at
                                .map(|t| t.elapsed().as_millis() as i64)
                                .unwrap_or_default();
                            let _ = insert_stage_event(
                                &state,
                                run_id,
                                "stt_final",
                                "ok",
                                Some(stt_ms),
                                None,
                                json!({
                                    "transcript_chars": transcript.chars().count(),
                                    "transcript_hash": content_hash(&transcript)
                                }),
                            )
                            .await;
                            let _ = sink.send(Message::Text(json!({
                                "type": "transcript.final",
                                "version": 1,
                                "run_id": run_id,
                                "client_run_id": client_run_id.as_deref(),
                                "text": transcript
                            }).to_string())).await;

                            let _ = sink.send(Message::Text(json!({
                                "type": "runtime.status",
                                "version": 1,
                                "run_id": run_id,
                                "client_run_id": client_run_id.as_deref(),
                                "phase": "polishing"
                            }).to_string())).await;

                            let model_used = selected_polish_model(&selected_model);
                            let polish_start = Instant::now();
                            match polish_runtime_transcript(
                                &state,
                                account_id,
                                run_id,
                                &transcript,
                                &output_language,
                                &selected_model,
                                screen_context.as_deref(),
                                &safe_vocab_terms,
                            )
                            .await
                            {
                                Ok(polished) => {
                                    let polish_ms = polish_start.elapsed().as_millis() as i64;
                                    let total_ms = stt_ms + polish_ms;
                                    let _ = update_runtime_session_result(
                                        &state,
                                        run_id,
                                        &transcript,
                                        &polished,
                                        json!({"stt_ms": stt_ms, "polish_ms": polish_ms, "total_ms": total_ms}),
                                    )
                                    .await;
                                    let _ = mark_runtime_session(&state, run_id, "completed", None).await;
                                    let _ = sink.send(Message::Text(json!({
                                        "type": "runtime.done",
                                        "version": 1,
                                        "run_id": run_id,
                                        "client_run_id": client_run_id.as_deref(),
                                        "output": polished,
                                        "transcript_hash": content_hash(&transcript),
                                        "model_used": model_used,
                                        "latency_ms": {
                                            "stt": stt_ms,
                                            "polish": polish_ms,
                                            "total": total_ms,
                                        }
                                    }).to_string())).await;
                                }
                                Err((status, body)) => {
                                    let _ = mark_runtime_session(&state, run_id, "failed", Some("polish_failed")).await;
                                    let _ = sink.send(Message::Text(runtime_error_payload(
                                        Some(run_id),
                                        client_run_id.as_deref(),
                                        "polish_failed",
                                        Some(status),
                                        Some(runtime_error_message(&body)),
                                    ).to_string())).await;
                                }
                            }
                        }
                    }
                    "audio.frame" => {
                        if let Some(pcm_b64) = value.get("pcm_b64").and_then(Value::as_str) {
                            match general_purpose::STANDARD.decode(pcm_b64) {
                                Ok(pcm) => {
                                    forward_audio_frame(
                                        &mut dg_sink,
                                        &state,
                                        active_run,
                                        pcm,
                                        &mut audio_frames,
                                        &mut audio_bytes,
                                    )
                                    .await;
                                }
                                Err(_) => {
                                    let _ = sink.send(Message::Text(json!({
                                        "type": "runtime.warning",
                                        "version": 1,
                                        "client_run_id": client_run_id.as_deref(),
                                        "message": "invalid audio.frame pcm_b64"
                                    }).to_string())).await;
                                }
                            }
                        }
                    }
                    "ping" => {
                        let _ = sink
                            .send(Message::Text(
                                json!({"type": "pong", "version": 1, "client_run_id": client_run_id.as_deref()}).to_string(),
                            ))
                            .await;
                    }
                    _ => {
                        let _ = sink
                            .send(Message::Text(
                                json!({
                                    "type": "runtime.warning",
                                    "version": 1,
                                    "client_run_id": client_run_id.as_deref(),
                                    "message": "unknown message type"
                                })
                                .to_string(),
                            ))
                            .await;
                    }
                }
            }
            Message::Binary(bytes) => {
                forward_audio_frame(
                    &mut dg_sink,
                    &state,
                    active_run,
                    bytes.to_vec(),
                    &mut audio_frames,
                    &mut audio_bytes,
                )
                .await;
            }
            Message::Close(_) => break,
            _ => {}
        }
            }

            dg_msg = async {
                match dg_stream.as_mut() {
                    Some(stream) => stream.next().await,
                    None => future::pending().await,
                }
            } => {
                let Some(dg_msg) = dg_msg else {
                    if let Some(run_id) = active_run {
                        let _ = insert_stage_event(
                            &state,
                            run_id,
                            "stt_ws_closed",
                            "warning",
                            None,
                            Some("deepgram_closed"),
                            json!({}),
                        )
                        .await;
                    }
                    dg_stream = None;
                    dg_sink = None;
                    continue;
                };
                match dg_msg {
                    Ok(DgMessage::Text(text)) => {
                        if let Some(event) = parse_deepgram_transcript_event(&text) {
                            if !saw_first_transcript_event {
                                saw_first_transcript_event = true;
                                if let Some(run_id) = active_run {
                                    let first_transcript_ms = stt_started_at
                                        .map(|t| t.elapsed().as_millis() as i64)
                                        .unwrap_or_default();
                                    let _ = insert_stage_event(
                                        &state,
                                        run_id,
                                        "stt_first_transcript",
                                        "ok",
                                        Some(first_transcript_ms),
                                        None,
                                        json!({
                                            "kind": if event.is_final { "final" } else { "partial" },
                                            "chars": event.transcript.chars().count()
                                        }),
                                    )
                                    .await;
                                }
                            }
                            if event.is_final {
                                transcript_segments.push(event.transcript.clone());
                                if let Some(run_id) = active_run {
                                    let _ = sink.send(Message::Text(json!({
                                        "type": "transcript.final",
                                        "version": 1,
                                        "run_id": run_id,
                                        "client_run_id": client_run_id.as_deref(),
                                        "text": event.transcript
                                    }).to_string())).await;
                                }
                            } else {
                                latest_partial = event.transcript.clone();
                                if let Some(run_id) = active_run {
                                    let _ = sink.send(Message::Text(json!({
                                        "type": "transcript.partial",
                                        "version": 1,
                                        "run_id": run_id,
                                        "client_run_id": client_run_id.as_deref(),
                                        "text": event.transcript
                                    }).to_string())).await;
                                }
                            }
                        }
                    }
                    Ok(DgMessage::Close(_)) => {
                        if let Some(run_id) = active_run {
                            let _ = insert_stage_event(
                                &state,
                                run_id,
                                "stt_ws_closed",
                                "warning",
                                None,
                                Some("deepgram_closed"),
                                json!({}),
                            ).await;
                        }
                        dg_stream = None;
                        dg_sink = None;
                    }
                    Err(e) => {
                        if let Some(run_id) = active_run {
                            let _ = insert_stage_event(
                                &state,
                                run_id,
                                "stt_ws_read",
                                "error",
                                None,
                                Some("deepgram_read_error"),
                                json!({"error": e.to_string().chars().take(240).collect::<String>()}),
                            ).await;
                        }
                        dg_stream = None;
                        dg_sink = None;
                    }
                    _ => {}
                }
            }
        }
    }

    if let Some(mut dg) = dg_sink {
        let _ = dg.close().await;
    }
}

async fn forward_audio_frame(
    dg_sink: &mut Option<DgSink>,
    state: &AppState,
    active_run: Option<Uuid>,
    pcm: Vec<u8>,
    audio_frames: &mut i64,
    audio_bytes: &mut i64,
) {
    *audio_frames += 1;
    *audio_bytes += pcm.len() as i64;
    if let Some(run_id) = active_run {
        if *audio_frames == 1 {
            let _ = insert_stage_event(
                state,
                run_id,
                "first_audio_frame",
                "ok",
                None,
                None,
                json!({"bytes": pcm.len()}),
            )
            .await;
        }
    }

    let Some(sink) = dg_sink.as_mut() else {
        if let Some(run_id) = active_run {
            let _ = insert_stage_event(
                state,
                run_id,
                "audio_frame_dropped",
                "warning",
                None,
                Some("deepgram_not_connected"),
                json!({"bytes": pcm.len()}),
            )
            .await;
        }
        return;
    };

    let send_result = sink.send(DgMessage::Binary(pcm)).await;
    if let Err(e) = send_result {
        if let Some(run_id) = active_run {
            let _ = insert_stage_event(
                state,
                run_id,
                "stt_ws_write",
                "error",
                None,
                Some("stt_write_failed"),
                json!({"error": e.to_string().chars().take(240).collect::<String>()}),
            )
            .await;
        }
    }
}

async fn drain_deepgram_finals(
    stream: &mut DgStream,
    transcript_segments: &mut Vec<String>,
    latest_partial: &mut String,
    max_wait: std::time::Duration,
) {
    let deadline = tokio::time::Instant::now() + max_wait;
    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            break;
        }
        let remaining = deadline - now;
        match tokio::time::timeout(remaining, stream.next()).await {
            Ok(Some(Ok(DgMessage::Text(text)))) => {
                if let Some(event) = parse_deepgram_transcript_event(&text) {
                    if event.is_final {
                        transcript_segments.push(event.transcript);
                    } else {
                        *latest_partial = event.transcript;
                    }
                }
            }
            Ok(Some(Ok(DgMessage::Close(_)))) | Ok(None) => break,
            Ok(Some(Err(_))) => break,
            Ok(Some(Ok(_))) => {}
            Err(_) => break,
        }
    }
}

struct DeepgramTranscriptEvent {
    transcript: String,
    is_final: bool,
}

fn parse_deepgram_transcript_event(text: &str) -> Option<DeepgramTranscriptEvent> {
    let raw: Value = serde_json::from_str(text).ok()?;
    let transcript = raw
        .get("channel")?
        .get("alternatives")?
        .as_array()?
        .first()?
        .get("transcript")?
        .as_str()?
        .trim();
    if transcript.is_empty() {
        return None;
    }
    let is_final = raw
        .get("is_final")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || raw
            .get("speech_final")
            .and_then(Value::as_bool)
            .unwrap_or(false);
    Some(DeepgramTranscriptEvent {
        transcript: transcript.to_string(),
        is_final,
    })
}

fn final_transcript(segments: &[String], latest_partial: &str) -> String {
    let joined = segments
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if !joined.trim().is_empty() {
        joined
    } else {
        latest_partial.trim().to_string()
    }
}

fn runtime_ws_start_metadata(
    value: &Value,
    sample_rate: u32,
    selected_model: &str,
    output_language: &str,
    safe_vocab_terms_count: usize,
    screen_context: Option<&str>,
) -> Value {
    let encoding = value
        .get("audio")
        .and_then(|audio| audio.get("encoding"))
        .and_then(Value::as_str)
        .unwrap_or("linear16");
    let channels = value
        .get("audio")
        .and_then(|audio| audio.get("channels"))
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .clamp(1, 8);
    json!({
        "endpoint": "voice_ws",
        "audio_runtime": "deepgram_mvp",
        "selected_model": selected_model,
        "output_language": output_language,
        "audio": {
            "encoding": encoding,
            "channels": channels,
            "sample_rate": sample_rate,
        },
        "safe_vocab_terms_count": safe_vocab_terms_count,
        "screen_context_chars": screen_context.map(|s| s.chars().count()).unwrap_or(0),
    })
}

fn runtime_error_message(body: &Json<Value>) -> String {
    body.0
        .get("message")
        .or_else(|| body.0.get("error"))
        .and_then(Value::as_str)
        .unwrap_or("runtime request failed")
        .chars()
        .take(240)
        .collect()
}

fn runtime_error_payload(
    run_id: Option<Uuid>,
    client_run_id: Option<&str>,
    error_kind: &str,
    status: Option<StatusCode>,
    message: Option<String>,
) -> Value {
    let mut payload = json!({
        "type": "runtime.error",
        "version": 1,
        "error_kind": error_kind,
    });
    if let Some(run_id) = run_id {
        payload["run_id"] = json!(run_id);
    }
    if let Some(client_run_id) = client_run_id {
        payload["client_run_id"] = json!(client_run_id);
    }
    if let Some(status) = status {
        payload["status"] = json!(status.as_u16());
    }
    if let Some(message) = message {
        payload["message"] = json!(message);
    }
    payload
}

/// Load the account's polish persona (tone_preset + custom_prompt) from
/// `runtime_user_settings` for the voice prompt. Best-effort: a missing row or a
/// query error must NEVER fail a dictation, so both fall back to the neutral
/// default that the voice path used before per-account tone existed.
/// Normalize a tone value onto the canonical said_core vocabulary — mirrors the mapping
/// inside `account_polish_persona` so a per-request tone override (e.g. the keyboard
/// rewrite) shares the same tone keys. Legacy mobile use-case names map across; canonical
/// and unknown values pass through (said_core maps anything unrecognized to neutral).
fn normalize_tone_preset(raw: &str) -> String {
    match raw {
        "work" | "email" => "professional".to_string(),
        "notes" => "concise".to_string(),
        other => other.to_string(),
    }
}

async fn account_polish_persona(state: &AppState, account_id: Uuid) -> (String, Option<String>) {
    let (tone_preset, custom_prompt) = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT tone_preset, custom_prompt FROM runtime_user_settings WHERE account_id = $1",
    )
    .bind(account_id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten()
    .unwrap_or_else(|| ("neutral".to_string(), None));
    // Map legacy mobile use-case tone names onto the canonical said_core vocabulary
    // the desktop already uses (the iOS picker matches these; this only rescues a
    // value persisted by an older client). Canonical + unknown values pass through —
    // said_core maps anything it doesn't recognise to neutral.
    let tone_preset = match tone_preset.as_str() {
        "work" | "email" => "professional".to_string(),
        "notes" => "concise".to_string(),
        _ => tone_preset,
    };
    (tone_preset, custom_prompt)
}

async fn polish_runtime_transcript(
    state: &AppState,
    account_id: Uuid,
    run_id: Uuid,
    transcript: &str,
    output_language: &str,
    selected_model: &str,
    screen_context: Option<&str>,
    safe_vocab_terms: &[String],
) -> Result<String, (StatusCode, Json<Value>)> {
    let formatted_transcript = crate::number_format::apply(transcript);
    if formatted_transcript != transcript {
        insert_stage_event(
            state,
            run_id,
            "formatter_pre",
            "ok",
            None,
            None,
            json!({
                "input_chars": transcript.chars().count(),
                "output_chars": formatted_transcript.chars().count()
            }),
        )
        .await?;
    }

    let (tone_preset, custom_prompt) = account_polish_persona(state, account_id).await;
    let prompt_start = Instant::now();
    let system_prompt = build_voice_system_prompt(
        output_language,
        &tone_preset,
        custom_prompt.as_deref(),
        screen_context,
        safe_vocab_terms,
    );
    let user_message = build_voice_user_message(&formatted_transcript, output_language);
    let prompt_ms = prompt_start.elapsed().as_millis() as i64;
    insert_stage_event(
        state,
        run_id,
        "prompt_built",
        "ok",
        Some(prompt_ms),
        None,
        json!({"prompt_version": "core-light-touch-2026-06-20"}),
    )
    .await?;

    let model = selected_polish_model(selected_model);
    let active_org_id = primary_org_id(state, account_id).await?;
    let credential = runtime_provider_secret(state, account_id, active_org_id, "groq").await?;
    let model_start = Instant::now();
    let output = call_groq(
        state,
        &credential.secret,
        model,
        &system_prompt,
        &user_message,
    )
    .await;
    let model_ms = model_start.elapsed().as_millis() as i64;

    match output {
        Ok(output) => {
            let _ = update_credential_used(state, credential.credential_id).await;
            insert_provider_usage(
                state,
                run_id,
                &credential,
                "groq",
                Some(model),
                Some(model_ms),
                "ok",
                None,
            )
            .await?;
            insert_stage_event(
                state,
                run_id,
                "llm_complete",
                "ok",
                Some(model_ms),
                None,
                json!({"model": model, "provider": "groq"}),
            )
            .await?;
            let output =
                crate::voice_polish_standalone::enforce_output_script(&output, output_language);
            let restored = restore_literal_tokens(&formatted_transcript, &output, safe_vocab_terms);
            let restored = restore_numeric_literal_tokens(&formatted_transcript, &restored);
            if restored != output {
                insert_stage_event(
                    state,
                    run_id,
                    "protected_resolver",
                    "ok",
                    None,
                    None,
                    json!({
                        "safe_vocab_terms": safe_vocab_terms.len(),
                        "changed": true
                    }),
                )
                .await?;
            }
            let formatted_output = crate::number_format::apply(&restored);
            let formatted_output =
                restore_numeric_literal_tokens(&formatted_transcript, &formatted_output);
            if formatted_output != restored {
                insert_stage_event(
                    state,
                    run_id,
                    "formatter_post",
                    "ok",
                    None,
                    None,
                    json!({
                        "input_chars": restored.chars().count(),
                        "output_chars": formatted_output.chars().count()
                    }),
                )
                .await?;
            }
            let email_output = crate::format_recover::recover_emails(&formatted_output);
            if email_output != formatted_output {
                insert_stage_event(
                    state,
                    run_id,
                    "email_recover_post",
                    "ok",
                    None,
                    None,
                    json!({
                        "input_chars": formatted_output.chars().count(),
                        "output_chars": email_output.chars().count()
                    }),
                )
                .await?;
            }
            Ok(email_output)
        }
        Err(err) => {
            let _ = insert_provider_usage(
                state,
                run_id,
                &credential,
                "groq",
                Some(model),
                Some(model_ms),
                "error",
                Some("model_failed"),
            )
            .await;
            let _ = insert_stage_event(
                state,
                run_id,
                "llm_complete",
                "error",
                Some(model_ms),
                Some("model_failed"),
                json!({"model": model, "provider": "groq"}),
            )
            .await;
            Err(err)
        }
    }
}

async fn update_runtime_session_result(
    state: &AppState,
    run_id: Uuid,
    input: &str,
    output: &str,
    latency_json: Value,
) -> Result<(), (StatusCode, Json<Value>)> {
    sqlx::query(
        "UPDATE runtime_sessions
            SET input_hash = $2,
                output_hash = $3,
                latency_json = $4,
                updated_at = now()
          WHERE id = $1",
    )
    .bind(run_id)
    .bind(content_hash(input))
    .bind(content_hash(output))
    .bind(latency_json)
    .execute(&state.db)
    .await
    .map_err(db_err)?;
    Ok(())
}

// ── WAV audio polish probe ──────────────────────────────────────────────────

pub async fn voice_wav(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Json(req): Json<VoiceWavRequest>,
) -> Result<Json<VoiceWavResponse>, (StatusCode, Json<Value>)> {
    let tenant_ctx = tenant::resolve_tenant(&state, &user, &headers).await?;
    if let Some(org_id) = tenant_ctx.active_org_id {
        org_quota::check_runtime_quota(&state, org_id).await?;
    }
    let total_start = Instant::now();
    let wav_data = general_purpose::STANDARD
        .decode(req.wav_b64.trim())
        .map_err(|_| json_error(StatusCode::BAD_REQUEST, "wav_b64 is not valid base64"))?;
    if wav_data.is_empty() {
        return Err(json_error(StatusCode::BAD_REQUEST, "wav_b64 is empty"));
    }
    let message_polish_mode = is_message_polish_wav_mode(&req.mode);
    let session_mode = if message_polish_mode {
        "message_polish"
    } else {
        "normal_voice"
    };
    let session_source = if message_polish_mode {
        "runtime_message_polish_audio"
    } else {
        "runtime_wav_probe"
    };

    let server_memory = load_runtime_memory(&state, user.account_id)
        .await
        .unwrap_or_default();
    let merged_vocab = merge_vocab_terms(&req.safe_vocab_terms, &server_memory.vocab_terms);

    let run_id = create_runtime_session(
        &state,
        user.account_id,
        tenant_ctx.active_org_id,
        req.client_run_id.as_deref(),
        session_mode,
        session_source,
        req.device_id.as_deref(),
        req.platform.as_deref(),
        req.app_version.as_deref(),
        json!({
            "endpoint": "voice_wav",
            "mode": session_mode,
            "wav_bytes": wav_data.len(),
            "safe_vocab_terms": merged_vocab.len(),
            "server_vocab_count": server_memory.vocab_terms.len(),
        }),
    )
    .await?;

    let stt_start = Instant::now();
    let (transcript, stt_provider_for_usage, stt_model, stt_credential) = if message_polish_mode {
        let credential_provider = "openai";
        let stt_credential = runtime_provider_secret(
            &state,
            user.account_id,
            tenant_ctx.active_org_id,
            credential_provider,
        )
        .await?;
        let model = openai_transcribe_model(&state);
        let transcript = match call_openai_audio_transcribe(
            &stt_credential.secret,
            &model,
            wav_data,
            session_source,
        )
        .await
        {
            Ok(transcript) => transcript,
            Err(e) => {
                let stt_ms = stt_start.elapsed().as_millis() as i64;
                let batch_err = "openai_transcribe_failed";
                let _ = insert_provider_usage(
                    &state,
                    run_id,
                    &stt_credential,
                    credential_provider,
                    Some(&model),
                    Some(stt_ms),
                    "error",
                    Some(batch_err),
                )
                .await;
                let _ = insert_stage_event(
                    &state,
                    run_id,
                    "stt_batch_complete",
                    "error",
                    Some(stt_ms),
                    Some(batch_err),
                    json!({
                        "provider": credential_provider,
                        "model": model,
                        "error": e.chars().take(240).collect::<String>()
                    }),
                )
                .await;
                let _ = mark_runtime_session(&state, run_id, "failed", Some(batch_err)).await;
                return Err(json_error(
                    StatusCode::BAD_GATEWAY,
                    &format!("OpenAI audio transcription failed: {e}"),
                ));
            }
        };
        (
            transcript,
            credential_provider.to_string(),
            model,
            stt_credential,
        )
    } else {
        let stt_provider = req
            .stt_provider
            .as_deref()
            .map(said_core::stt::resolve_provider_from_pref)
            .unwrap_or_else(|| state.stt_provider.clone());
        let credential_provider = runtime_stt_credential_provider(&stt_provider);
        let stt_credential = runtime_provider_secret(
            &state,
            user.account_id,
            tenant_ctx.active_org_id,
            credential_provider,
        )
        .await?;
        let stt_model = "nova-3".to_string();
        let transcript = match stt::call_batch_stt(
            &stt_provider,
            &stt_credential.secret,
            wav_data,
            session_source,
        )
        .await
        {
            Ok(transcript) => transcript,
            Err(e) => {
                let stt_ms = stt_start.elapsed().as_millis() as i64;
                let batch_err = format!("{credential_provider}_batch_failed");
                let _ = insert_provider_usage(
                    &state,
                    run_id,
                    &stt_credential,
                    credential_provider,
                    Some(&stt_model),
                    Some(stt_ms),
                    "error",
                    Some(&batch_err),
                )
                .await;
                let _ = insert_stage_event(
                    &state,
                    run_id,
                    "stt_batch_complete",
                    "error",
                    Some(stt_ms),
                    Some(&batch_err),
                    json!({
                        "provider": credential_provider,
                        "model": stt_model,
                        "error": e.chars().take(240).collect::<String>()
                    }),
                )
                .await;
                let _ = mark_runtime_session(&state, run_id, "failed", Some(&batch_err)).await;
                return Err(json_error(
                    StatusCode::BAD_GATEWAY,
                    &format!("{credential_provider} batch STT failed: {e}"),
                ));
            }
        };
        (
            transcript,
            credential_provider.to_string(),
            stt_model,
            stt_credential,
        )
    };
    let stt_ms = stt_start.elapsed().as_millis() as i64;
    update_credential_used(&state, stt_credential.credential_id).await?;
    insert_provider_usage(
        &state,
        run_id,
        &stt_credential,
        &stt_provider_for_usage,
        Some(&stt_model),
        Some(stt_ms),
        "ok",
        None,
    )
    .await?;
    insert_stage_event(
        &state,
        run_id,
        "stt_batch_complete",
        "ok",
        Some(stt_ms),
        None,
        json!({
            "provider": stt_provider_for_usage,
            "model": stt_model,
            "transcript_chars": transcript.chars().count(),
            "transcript_hash": content_hash(&transcript)
        }),
    )
    .await?;

    let polish_start = Instant::now();
    let (output, model, prompt_version, history_source) = if message_polish_mode {
        if state.deepseek_api_key.trim().is_empty() {
            let _ = mark_runtime_session(
                &state,
                run_id,
                "failed",
                Some("message_polish_unconfigured"),
            )
            .await;
            return Err(json_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "DEEPSEEK_API_KEY is not configured on the server",
            ));
        }
        let system_prompt = build_message_polish_system_prompt();
        let user_message = build_message_polish_user_message(&transcript);
        let raw_output = match call_deepseek_message_polish(
            &state,
            &state.deepseek_api_key,
            &system_prompt,
            &user_message,
        )
        .await
        {
            Ok(output) => output,
            Err(err) => {
                let _ =
                    mark_runtime_session(&state, run_id, "failed", Some("message_polish_failed"))
                        .await;
                return Err(err);
            }
        };
        let output = scrub_message_polish_output(&raw_output);
        let model = deepseek_message_polish_model(&state);
        insert_stage_event(
            &state,
            run_id,
            "message_polish_model",
            "ok",
            None,
            None,
            json!({
                "model": model,
                "input_chars": transcript.chars().count(),
                "output_chars": output.chars().count(),
                "source": "audio"
            }),
        )
        .await?;
        (
            output,
            model,
            "message-polish-audio-openai-transcribe-deepseek-v4-flash-2026-06-20".to_string(),
            "server_message_polish_audio",
        )
    } else {
        let output = match polish_runtime_transcript(
            &state,
            user.account_id,
            run_id,
            &transcript,
            &req.output_language,
            &req.selected_model,
            req.screen_context.as_deref(),
            &merged_vocab,
        )
        .await
        {
            Ok(output) => output,
            Err(err) => {
                let _ = mark_runtime_session(&state, run_id, "failed", Some("polish_failed")).await;
                return Err(err);
            }
        };

        // Stable 2.3.4 parity: final runtime mutation is approved/safe exact STT
        // aliases only. Edit-policy rules remain memory/status data for now; they
        // must not broaden the server output until parity against the shipped
        // local pipeline is proven.
        let formatted_for_resolver = crate::number_format::apply(&transcript);
        let (output, resolver_applied, resolver_skipped) =
            apply_exact_resolver(&output, &formatted_for_resolver, &server_memory);
        if resolver_applied > 0 {
            let _ = insert_stage_event(
                &state,
                run_id,
                "exact_resolver",
                "ok",
                None,
                None,
                json!({
                    "evidence_count": server_memory.replacements.len(),
                    "applied_count": resolver_applied,
                    "skipped_count": resolver_skipped,
                }),
            )
            .await;
        }

        (
            output,
            selected_polish_model(&req.selected_model).to_string(),
            "server-runtime-wav-probe-2026-06-07".to_string(),
            "server_wav",
        )
    };

    let polish_ms = polish_start.elapsed().as_millis() as i64;
    let total_ms = total_start.elapsed().as_millis() as i64;
    update_runtime_session_result(
        &state,
        run_id,
        &transcript,
        &output,
        json!({"stt_ms": stt_ms, "polish_ms": polish_ms, "total_ms": total_ms}),
    )
    .await?;
    mark_runtime_session(&state, run_id, "completed", None).await?;

    let org_id_for_history = primary_org_id(&state, user.account_id).await.ok().flatten();
    crate::routes::runtime_history::write_history_from_runtime(
        &state,
        user.account_id,
        org_id_for_history,
        run_id,
        req.client_run_id.as_deref(),
        req.recording_id.as_deref(),
        &transcript,
        &output,
        &model,
        history_source,
        Some(stt_ms),
        Some(polish_ms),
    )
    .await;

    Ok(Json(VoiceWavResponse {
        run_id: run_id.to_string(),
        transcript_hash: content_hash(&transcript),
        transcript,
        output,
        model_used: model,
        prompt_version,
        latency_ms: RuntimeAudioLatency {
            stt: stt_ms,
            polish: polish_ms,
            total: total_ms,
        },
    }))
}

// DeepSeek base-url + model are read once at startup into AppState
// (deepseek_base_url / deepseek_message_polish_model).

fn build_message_polish_system_prompt() -> String {
    "You are a stateless text processing utility. Your sole function is to transform input text into a professional English format.\n\n\
     Execution Rules:\n\n\
     No Dialogue: Do NOT answer questions. Do NOT ask for context. Do NOT provide \"Introduction Mode\" unless the input is specifically \"Hello\" or \"Who are you?\".\n\n\
     Handle Questions as Data: If the user provides a question (e.g., \"What went wrong?\"), do NOT answer it. Instead, rephrase it into a formal professional inquiry (e.g., \"Please provide a detailed explanation regarding the cause of the discrepancy.\").\n\n\
     Translation: Automatically detect Hindi/Hinglish and translate to English before rephrasing.\n\n\
     Tone: Always use a clear, polite, and professional tone.\n\n\
     Readable Formatting: Make the output ready to send to another person. Do NOT return a long wall of text when the message contains multiple ideas.\n\n\
     Paragraphing: For outputs longer than about 45 words, split into short paragraphs of 1-3 sentences each. Put a blank line between paragraphs.\n\n\
     Lists: If the input contains multiple action items, issues, requirements, or questions, use concise bullet points. Do not use bullets for a simple one-topic message.\n\n\
     Preserve Structure: Preserve meaningful line breaks from the input when they help readability, but clean them up professionally.\n\n\
     Short Messages: If the input is short and naturally one idea, keep it as a single polished paragraph.\n\n\
     Output Format (Strict): Return ONLY the final rephrased text, including useful paragraph breaks or bullets when appropriate.\n\n\
     No quotation marks.\n\n\
     No introductory phrases (e.g., \"Here is the rephrased version\").\n\n\
     No conversational filler.\n\n\
     Input-to-Output Examples:\n\n\
     Input: \"What went wrong and why\"\n\n\
     Output: Could you please provide a detailed explanation regarding the root cause of these issues?\n\n\
     Input: \"kaam kab tak khatam hoga?\"\n\n\
     Output: Could you please provide an estimated timeline for the completion of the task?"
        .to_string()
}

fn build_message_polish_user_message(text: &str) -> String {
    text.to_string()
}

fn scrub_message_polish_output(output: &str) -> String {
    let trimmed = output.trim();
    for prefix in [
        "Explanation:",
        "Previous output:",
        "Here is the rephrased version:",
        "Rephrased version:",
    ] {
        if trimmed.starts_with(prefix) {
            return trimmed[prefix.len()..].trim().to_string();
        }
    }
    trimmed.to_string()
}

fn openai_transcribe_model(state: &AppState) -> String {
    let configured = state.openai_transcribe_model.trim();
    if configured.is_empty() {
        DEFAULT_OPENAI_TRANSCRIBE_MODEL.to_string()
    } else {
        configured.to_string()
    }
}

fn deepseek_message_polish_model(state: &AppState) -> String {
    let configured = state.deepseek_message_polish_model.trim();
    if configured.is_empty() {
        DEFAULT_DEEPSEEK_MESSAGE_POLISH_MODEL.to_string()
    } else {
        configured.to_string()
    }
}

async fn call_openai_audio_transcribe(
    api_key: &str,
    model: &str,
    wav_data: Vec<u8>,
    tag: &str,
) -> Result<String, String> {
    let file = reqwest::multipart::Part::bytes(wav_data)
        .file_name("airnote-message-polish.wav")
        .mime_str("audio/wav")
        .map_err(|e| format!("{tag}: failed to build OpenAI audio part: {e}"))?;
    let form = reqwest::multipart::Form::new()
        .text("model", model.to_string())
        .text("response_format", "json")
        .part("file", file);

    let client = &*crate::HTTP_CLIENT;
    let resp = client
        .post(OPENAI_AUDIO_TRANSCRIPTIONS_ENDPOINT)
        .bearer_auth(api_key)
        .multipart(form)
        .timeout(std::time::Duration::from_secs(60))
        .send()
        .await
        .map_err(|e| format!("{tag}: OpenAI transcription request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "{tag}: OpenAI transcription returned {status}: {}",
            &body[..body.len().min(300)]
        ));
    }

    let raw = resp
        .json::<Value>()
        .await
        .map_err(|e| format!("{tag}: failed to parse OpenAI transcription response: {e}"))?;
    let transcript = raw
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();

    if transcript.is_empty() {
        Err(format!("{tag}: OpenAI returned empty transcript"))
    } else {
        tracing::info!(
            "[runtime] OpenAI message-polish transcription ok model={} chars={}",
            model,
            transcript.chars().count()
        );
        Ok(transcript)
    }
}

async fn call_deepseek_message_polish(
    state: &AppState,
    api_key: &str,
    system_prompt: &str,
    user_message: &str,
) -> Result<String, (StatusCode, Json<Value>)> {
    let model = deepseek_message_polish_model(state);
    let url = format!("{}/v1/chat/completions", state.deepseek_base_url);
    let estimated_input_tokens = user_message.len() / 4;
    let max_tokens = (estimated_input_tokens * 2 + 256).min(4096);
    let body = json!({
        "model": model,
        "temperature": 0.0,
        "top_p": 0.9,
        "max_tokens": max_tokens,
        "stream": false,
        "thinking": { "type": "disabled" },
        "messages": [
            { "role": "system", "content": system_prompt },
            { "role": "user", "content": user_message }
        ]
    });

    let client = &*crate::HTTP_CLIENT;
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .timeout(std::time::Duration::from_secs(60))
        .send()
        .await
        .map_err(|e| {
            json_error(
                StatusCode::BAD_GATEWAY,
                &format!("DeepSeek message polish request failed: {e}"),
            )
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let preview = resp.text().await.unwrap_or_default();
        tracing::warn!(
            "[runtime] DeepSeek HTTP {status}: {}",
            &preview[..preview.len().min(300)]
        );
        return Err(json_error(
            StatusCode::BAD_GATEWAY,
            &format!("DeepSeek returned {status}"),
        ));
    }

    let value: Value = resp.json().await.map_err(|e| {
        json_error(
            StatusCode::BAD_GATEWAY,
            &format!("DeepSeek response parse failed: {e}"),
        )
    })?;

    let output = value
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    if output.is_empty() {
        return Err(json_error(
            StatusCode::BAD_GATEWAY,
            "DeepSeek returned empty output",
        ));
    }

    Ok(output)
}

// ── Message polish (DeepSeek) ───────────────────────────────────────────────

pub async fn message_polish(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Json(req): Json<MessagePolishRequest>,
) -> Result<Json<MessagePolishResponse>, (StatusCode, Json<Value>)> {
    let tenant_ctx = tenant::resolve_tenant(&state, &user, &headers).await?;
    let total_start = Instant::now();
    let text = req.text.trim();
    if text.is_empty() {
        return Err(json_error(StatusCode::BAD_REQUEST, "text is required"));
    }

    if state.deepseek_api_key.trim().is_empty() {
        return Err(json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "DEEPSEEK_API_KEY is not configured on the server",
        ));
    }

    let run_id = create_runtime_session(
        &state,
        user.account_id,
        tenant_ctx.active_org_id,
        req.client_run_id.as_deref(),
        "message_polish",
        "desktop_message_polish",
        None,
        None,
        None,
        json!({
            "endpoint": "message_polish",
            "input_chars": text.chars().count(),
        }),
    )
    .await?;

    let prompt_start = Instant::now();
    let system_prompt = build_message_polish_system_prompt();
    let user_message = build_message_polish_user_message(text);
    let prompt_ms = prompt_start.elapsed().as_millis() as i64;

    let model = deepseek_message_polish_model(&state);
    let model_start = Instant::now();
    let raw_output = call_deepseek_message_polish(
        &state,
        &state.deepseek_api_key,
        &system_prompt,
        &user_message,
    )
    .await?;
    let output = scrub_message_polish_output(&raw_output);
    let model_ms = model_start.elapsed().as_millis() as i64;
    let total_ms = total_start.elapsed().as_millis() as i64;

    tracing::info!(
        "[runtime] message polish done account={} run_id={} model={} output_chars={} model_ms={} total_ms={}",
        user.account_id,
        run_id,
        model,
        output.len(),
        model_ms,
        total_ms,
    );

    // Telemetry stage event is non-essential to the response — write it after
    // returning so it never blocks the polished text (#5).
    {
        let bg_state = state.clone();
        let bg_model = model.clone();
        let input_chars = text.chars().count();
        let output_chars = output.chars().count();
        tokio::spawn(async move {
            let _ = insert_stage_event(
                &bg_state,
                run_id,
                "message_polish_model",
                "ok",
                None,
                None,
                json!({
                    "model": bg_model,
                    "input_chars": input_chars,
                    "output_chars": output_chars,
                }),
            )
            .await;
        });
    }

    Ok(Json(MessagePolishResponse {
        run_id: run_id.to_string(),
        output,
        model_used: model,
        prompt_version: "message-polish-deepseek-2026-06-09".to_string(),
        latency_ms: RuntimeLatency {
            prompt: prompt_ms,
            model: model_ms,
            total: total_ms,
        },
    }))
}

// ── Transcript-only polish probe ────────────────────────────────────────────

pub async fn voice_polish(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Json(req): Json<VoicePolishRequest>,
) -> Result<Json<VoicePolishResponse>, (StatusCode, Json<Value>)> {
    let tenant_ctx = tenant::resolve_tenant(&state, &user, &headers).await?;
    if let Some(org_id) = tenant_ctx.active_org_id {
        org_quota::check_runtime_quota(&state, org_id).await?;
    }
    let total_start = Instant::now();
    let transcript = req.transcript.trim();
    if transcript.is_empty() {
        return Err(json_error(
            StatusCode::BAD_REQUEST,
            "transcript is required",
        ));
    }

    let server_memory = load_runtime_memory(&state, user.account_id)
        .await
        .unwrap_or_default();
    let merged_vocab = merge_vocab_terms(&req.safe_vocab_terms, &server_memory.vocab_terms);

    let run_id = create_runtime_session(
        &state,
        user.account_id,
        tenant_ctx.active_org_id,
        req.client_run_id.as_deref(),
        "normal_voice",
        "desktop_voice",
        None,
        None,
        None,
        json!({
            "endpoint": "voice_polish_probe",
            "transcript_chars": transcript.chars().count(),
            "safe_vocab_terms": merged_vocab.len(),
            "server_vocab_count": server_memory.vocab_terms.len(),
        }),
    )
    .await?;

    let prompt_start = Instant::now();
    let formatted_transcript = crate::number_format::apply(transcript);
    if formatted_transcript != transcript {
        // Telemetry only — fire-and-forget so it never gates the model call (#4).
        let bg = state.clone();
        let input_chars = transcript.chars().count();
        let output_chars = formatted_transcript.chars().count();
        tokio::spawn(async move {
            let _ = insert_stage_event(
                &bg,
                run_id,
                "formatter_pre",
                "ok",
                None,
                None,
                json!({"input_chars": input_chars, "output_chars": output_chars}),
            )
            .await;
        });
    }
    // An explicit per-request tone (only the iOS keyboard "select → polish" sends one)
    // marks a REWRITE: rephrase freely + translate strictly into the chosen language.
    // No tone = the dictation path, which preserves the speaker's language and words.
    let explicit_tone = req
        .tone_preset
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty());
    let is_rewrite = explicit_tone.is_some();
    let (tone_preset, custom_prompt) = match explicit_tone {
        Some(raw) => (normalize_tone_preset(raw), None),
        None => account_polish_persona(&state, user.account_id).await,
    };
    let (system_prompt, user_message) = if is_rewrite {
        (
            build_rewrite_system_prompt(&tone_preset, &req.output_language),
            build_rewrite_user_message(&formatted_transcript, &req.output_language),
        )
    } else {
        (
            build_voice_system_prompt(
                &req.output_language,
                &tone_preset,
                custom_prompt.as_deref(),
                req.screen_context.as_deref(),
                &merged_vocab,
            ),
            build_voice_user_message(&formatted_transcript, &req.output_language),
        )
    };
    let prompt_ms = prompt_start.elapsed().as_millis() as i64;

    let model = selected_polish_model(&req.selected_model);
    let credential =
        match runtime_provider_secret(&state, user.account_id, tenant_ctx.active_org_id, "groq")
            .await
        {
            Ok(credential) => credential,
            Err(err) => {
                let _ = insert_stage_event(
                    &state,
                    run_id,
                    "credential_lookup",
                    "error",
                    None,
                    Some("provider_credential_missing"),
                    json!({"provider": "groq"}),
                )
                .await;
                let _ = mark_runtime_session(
                    &state,
                    run_id,
                    "failed",
                    Some("provider_credential_missing"),
                )
                .await;
                return Err(err);
            }
        };

    tracing::info!(
        "[runtime] voice polish start account={} run_id={} model={} credential_scope={} transcript_chars={} vocab_hints={}",
        user.account_id,
        run_id,
        model,
        credential.scope,
        transcript.len(),
        merged_vocab.len(),
    );

    {
        // Telemetry only — fire-and-forget so it never gates the model call (#4).
        let bg = state.clone();
        tokio::spawn(async move {
            let _ = insert_stage_event(
                &bg,
                run_id,
                "prompt_built",
                "ok",
                Some(prompt_ms),
                None,
                json!({"prompt_version": "core-light-touch-2026-06-20"}),
            )
            .await;
        });
    }

    let model_start = Instant::now();
    let output = polish_llm(
        &state,
        tenant_ctx.active_org_id,
        &credential.secret,
        model,
        &system_prompt,
        &user_message,
    )
    .await;
    let model_ms = model_start.elapsed().as_millis() as i64;
    let total_ms = total_start.elapsed().as_millis() as i64;

    let output = match output {
        Ok(output) => output,
        Err(err) => {
            let _ = insert_stage_event(
                &state,
                run_id,
                "llm_complete",
                "error",
                Some(model_ms),
                Some("model_failed"),
                json!({}),
            )
            .await;
            let _ = insert_provider_usage(
                &state,
                run_id,
                &credential,
                "groq",
                Some(model),
                Some(model_ms),
                "error",
                Some("model_failed"),
            )
            .await;
            let _ = mark_runtime_session(&state, run_id, "failed", Some("model_failed")).await;
            return Err(err);
        }
    };

    // Deterministic post-processing runs inline (it computes the final output),
    // but its telemetry stage events are RECORDED here and written later — every
    // DB write is deferred to a single post-response spawn so nothing blocks the
    // polished text returning to the client (#2).
    let mut deferred_events: Vec<(&'static str, Option<i64>, Value)> = Vec::new();

    let output =
        crate::voice_polish_standalone::enforce_output_script(&output, &req.output_language);
    let restored = restore_literal_tokens(&formatted_transcript, &output, &merged_vocab);
    let restored = restore_numeric_literal_tokens(&formatted_transcript, &restored);
    if restored != output {
        deferred_events.push((
            "protected_resolver",
            None,
            json!({"safe_vocab_terms": merged_vocab.len(), "changed": true}),
        ));
    }
    let output = crate::number_format::apply(&restored);
    let output = restore_numeric_literal_tokens(&formatted_transcript, &output);
    if output != restored {
        deferred_events.push((
            "formatter_post",
            None,
            json!({
                "input_chars": restored.chars().count(),
                "output_chars": output.chars().count()
            }),
        ));
    }
    let email_output = crate::format_recover::recover_emails(&output);
    let output = if email_output != output {
        deferred_events.push((
            "email_recover_post",
            None,
            json!({
                "input_chars": output.chars().count(),
                "output_chars": email_output.chars().count()
            }),
        ));
        email_output
    } else {
        output
    };

    // Stable 2.3.4 parity: final runtime mutation is approved/safe exact STT
    // aliases only. Edit-policy rules are intentionally not applied here.
    let (output, resolver_applied, resolver_skipped) =
        apply_exact_resolver(&output, &formatted_transcript, &server_memory);
    if resolver_applied > 0 {
        deferred_events.push((
            "exact_resolver",
            None,
            json!({
                "evidence_count": server_memory.replacements.len(),
                "applied_count": resolver_applied,
                "skipped_count": resolver_skipped,
            }),
        ));
    }
    // Deterministic sentence-case + terminal punctuation. The light-touch polish
    // prompt (anti-"Scout meltdown") intentionally under-edits casing/punctuation,
    // so guarantee them mechanically here — never re-triggers LLM over-editing.
    let output = tidy_casing(&output);

    deferred_events.push(("llm_complete", Some(model_ms), json!({"model": model})));

    tracing::info!(
        "[runtime] voice polish done account={} run_id={} model={} output_chars={} model_ms={} total_ms={}",
        user.account_id,
        run_id,
        model,
        output.len(),
        model_ms,
        total_ms,
    );

    // Defer all telemetry/billing/history writes off the response path (#2/#3).
    // create_runtime_session already committed the parent row (run_id is in the
    // response), so these children satisfy their FKs. Errors are logged, never
    // surfaced — a telemetry write must not turn a successful polish into a 500.
    {
        let bg_state = state.clone();
        let bg_credential = credential.clone();
        let bg_transcript = transcript.to_string();
        let bg_output = output.clone();
        let bg_client_run_id = req.client_run_id.clone();
        let bg_account_id = user.account_id;
        // #3: reuse the org already resolved + membership-validated by
        // resolve_tenant instead of re-deriving it via primary_org_id.
        let org_id_for_history = tenant_ctx.active_org_id;
        tokio::spawn(async move {
            let _ = update_credential_used(&bg_state, bg_credential.credential_id).await;
            let _ = insert_provider_usage(
                &bg_state,
                run_id,
                &bg_credential,
                "groq",
                Some(model),
                Some(model_ms),
                "ok",
                None,
            )
            .await;
            for (name, latency_ms, payload) in deferred_events {
                let _ =
                    insert_stage_event(&bg_state, run_id, name, "ok", latency_ms, None, payload)
                        .await;
            }
            let _ = mark_runtime_session(&bg_state, run_id, "completed", None).await;
            crate::routes::runtime_history::write_history_from_runtime(
                &bg_state,
                bg_account_id,
                org_id_for_history,
                run_id,
                bg_client_run_id.as_deref(),
                None,
                &bg_transcript,
                &bg_output,
                model,
                "server_polish",
                None,
                Some(model_ms),
            )
            .await;
        });
    }

    Ok(Json(VoicePolishResponse {
        run_id: run_id.to_string(),
        output,
        model_used: model.to_string(),
        prompt_version: "core-light-touch-2026-06-20".to_string(),
        latency_ms: RuntimeLatency {
            prompt: prompt_ms,
            model: model_ms,
            total: total_ms,
        },
    }))
}

// ── Persistence helpers ─────────────────────────────────────────────────────

async fn create_runtime_session(
    state: &AppState,
    account_id: Uuid,
    active_org_id: Option<Uuid>,
    client_run_id: Option<&str>,
    mode: &str,
    source: &str,
    device_id: Option<&str>,
    platform: Option<&str>,
    app_version: Option<&str>,
    metadata: Value,
) -> Result<Uuid, (StatusCode, Json<Value>)> {
    let org_id = active_org_id;
    let run_id: Uuid = sqlx::query_scalar(
        "INSERT INTO runtime_sessions
            (account_id, org_id, device_id, client_run_id, mode, source, platform, app_version,
             status, metadata_json)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'created', $9)
         RETURNING id",
    )
    .bind(account_id)
    .bind(org_id)
    .bind(device_id)
    .bind(client_run_id)
    .bind(mode)
    .bind(source)
    .bind(platform)
    .bind(app_version)
    .bind(metadata)
    .fetch_one(&state.db)
    .await
    .map_err(db_err)?;

    Ok(run_id)
}

async fn insert_stage_event(
    state: &AppState,
    run_id: Uuid,
    stage: &str,
    status: &str,
    latency_ms: Option<i64>,
    error_kind: Option<&str>,
    metadata: Value,
) -> Result<(), (StatusCode, Json<Value>)> {
    sqlx::query(
        "INSERT INTO runtime_stage_events
            (run_id, stage, status, latency_ms, error_kind, metadata_json)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(run_id)
    .bind(stage)
    .bind(status)
    .bind(latency_ms)
    .bind(error_kind)
    .bind(metadata)
    .execute(&state.db)
    .await
    .map_err(db_err)?;
    Ok(())
}

async fn mark_runtime_session(
    state: &AppState,
    run_id: Uuid,
    status: &str,
    error_kind: Option<&str>,
) -> Result<(), (StatusCode, Json<Value>)> {
    sqlx::query(
        "UPDATE runtime_sessions
            SET status = $2, error_kind = $3, updated_at = now()
          WHERE id = $1",
    )
    .bind(run_id)
    .bind(status)
    .bind(error_kind)
    .execute(&state.db)
    .await
    .map_err(db_err)?;
    Ok(())
}

async fn primary_org_id(
    state: &AppState,
    account_id: Uuid,
) -> Result<Option<Uuid>, (StatusCode, Json<Value>)> {
    tenant::resolve_ws_org_id(state, account_id).await
}

// ── Crypto helpers ──────────────────────────────────────────────────────────

struct EncryptedSecret {
    ciphertext: String,
    nonce: String,
}

fn encrypt_secret(
    state: &AppState,
    secret: &str,
) -> Result<EncryptedSecret, (StatusCode, Json<Value>)> {
    let cipher = runtime_cipher(state)?;
    let mut nonce_bytes = [0u8; 12];
    rand::rngs::OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher.encrypt(nonce, secret.as_bytes()).map_err(|_| {
        json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to encrypt provider credential",
        )
    })?;
    Ok(EncryptedSecret {
        ciphertext: general_purpose::STANDARD.encode(ciphertext),
        nonce: general_purpose::STANDARD.encode(nonce_bytes),
    })
}

fn decrypt_secret(
    state: &AppState,
    ciphertext: &str,
    nonce: &str,
) -> Result<String, (StatusCode, Json<Value>)> {
    let cipher = runtime_cipher(state)?;
    let ciphertext = general_purpose::STANDARD
        .decode(ciphertext)
        .map_err(|_| json_error(StatusCode::BAD_REQUEST, "invalid credential ciphertext"))?;
    let nonce_bytes = general_purpose::STANDARD
        .decode(nonce)
        .map_err(|_| json_error(StatusCode::BAD_REQUEST, "invalid credential nonce"))?;
    if nonce_bytes.len() != 12 {
        return Err(json_error(
            StatusCode::BAD_REQUEST,
            "invalid credential nonce length",
        ));
    }
    let plaintext = cipher
        .decrypt(Nonce::from_slice(&nonce_bytes), ciphertext.as_ref())
        .map_err(|_| json_error(StatusCode::BAD_REQUEST, "credential decrypt failed"))?;
    String::from_utf8(plaintext)
        .map_err(|_| json_error(StatusCode::BAD_REQUEST, "credential is not valid UTF-8"))
}

/// Derive the AES-256-GCM cipher from the raw credentials key. Called once at
/// startup (see `main.rs`) and cached in `AppState.runtime_cipher`. Returns
/// None when the key is unconfigured / too short.
pub fn derive_runtime_cipher(secret: &str) -> Option<Aes256Gcm> {
    let secret = secret.trim();
    if secret.len() < 16 {
        return None;
    }
    let key = Sha256::digest(secret.as_bytes());
    Some(Aes256Gcm::new_from_slice(&key).expect("sha256 produces 32-byte key"))
}

fn runtime_cipher(state: &AppState) -> Result<Aes256Gcm, (StatusCode, Json<Value>)> {
    state.runtime_cipher.clone().ok_or_else(|| {
        json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "RUNTIME_CREDENTIALS_KEY is not configured",
        )
    })
}

fn last4(secret: &str) -> String {
    let chars = secret.chars().collect::<Vec<_>>();
    let start = chars.len().saturating_sub(4);
    chars[start..].iter().collect()
}

// ── DB rows ─────────────────────────────────────────────────────────────────

#[derive(sqlx::FromRow)]
struct CredentialRow {
    id: Uuid,
    provider: String,
    scope: String,
    org_id: Option<Uuid>,
    account_id: Option<Uuid>,
    display_name: String,
    secret_last4: String,
    status: String,
    validated_at: Option<chrono::DateTime<chrono::Utc>>,
    last_used_at: Option<chrono::DateTime<chrono::Utc>>,
    last_error: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(sqlx::FromRow)]
struct CredentialSecretRow {
    id: Uuid,
    provider: String,
    org_id: Option<Uuid>,
    account_id: Option<Uuid>,
    secret_ciphertext: String,
    secret_nonce: String,
}

#[derive(sqlx::FromRow)]
struct CredentialSecretWithScopeRow {
    id: Uuid,
    scope: String,
    secret_ciphertext: String,
    secret_nonce: String,
}

#[derive(sqlx::FromRow)]
struct RuntimeRunRow {
    id: Uuid,
    account_id: Uuid,
    account_email: String,
    client_run_id: Option<String>,
    mode: String,
    source: String,
    platform: Option<String>,
    app_version: Option<String>,
    status: String,
    error_kind: Option<String>,
    input_hash: Option<String>,
    output_hash: Option<String>,
    provider_summary: Value,
    latency_json: Value,
    metadata_json: Value,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(sqlx::FromRow)]
struct RuntimeLearningEventRow {
    id: Uuid,
    account_id: Uuid,
    account_email: String,
    run_id: Option<Uuid>,
    recording_id: Option<String>,
    event_type: String,
    classification: Option<String>,
    input_hash: Option<String>,
    output_hash: Option<String>,
    corrected_hash: Option<String>,
    payload_json: Value,
    server_judgment: Value,
    created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(sqlx::FromRow)]
struct RuntimeStageRow {
    id: Uuid,
    stage: String,
    status: String,
    latency_ms: Option<i64>,
    error_kind: Option<String>,
    metadata_json: Value,
    created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(sqlx::FromRow)]
struct RuntimeProviderUsageRow {
    id: Uuid,
    provider: String,
    model: Option<String>,
    credential_scope: String,
    request_ms: Option<i64>,
    ttft_ms: Option<i64>,
    stream_ms: Option<i64>,
    total_ms: Option<i64>,
    timeout_ms: Option<i64>,
    status: String,
    error_kind: Option<String>,
    fallback_reason: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
}

impl From<CredentialRow> for CredentialSummary {
    fn from(row: CredentialRow) -> Self {
        Self {
            id: row.id,
            provider: row.provider,
            scope: row.scope,
            org_id: row.org_id,
            account_id: row.account_id,
            display_name: row.display_name,
            secret_last4: row.secret_last4,
            status: row.status,
            validated_at: row.validated_at,
            last_used_at: row.last_used_at,
            last_error: row.last_error,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

impl From<RuntimeRunRow> for RuntimeRunSummary {
    fn from(row: RuntimeRunRow) -> Self {
        Self {
            id: row.id,
            account_id: row.account_id,
            account_email: row.account_email,
            client_run_id: row.client_run_id,
            mode: row.mode,
            source: row.source,
            platform: row.platform,
            app_version: row.app_version,
            status: row.status,
            error_kind: row.error_kind,
            input_hash: row.input_hash,
            output_hash: row.output_hash,
            provider_summary: row.provider_summary,
            latency_json: row.latency_json,
            metadata_json: row.metadata_json,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

impl From<RuntimeLearningEventRow> for RuntimeLearningEventSummary {
    fn from(row: RuntimeLearningEventRow) -> Self {
        Self {
            id: row.id,
            account_id: row.account_id,
            account_email: row.account_email,
            run_id: row.run_id,
            recording_id: row.recording_id,
            event_type: row.event_type,
            classification: row.classification,
            input_hash: row.input_hash,
            output_hash: row.output_hash,
            corrected_hash: row.corrected_hash,
            payload_json: row.payload_json,
            server_judgment: row.server_judgment,
            created_at: row.created_at,
        }
    }
}

impl From<RuntimeStageRow> for RuntimeStageSummary {
    fn from(row: RuntimeStageRow) -> Self {
        Self {
            id: row.id,
            stage: row.stage,
            status: row.status,
            latency_ms: row.latency_ms,
            error_kind: row.error_kind,
            metadata_json: row.metadata_json,
            created_at: row.created_at,
        }
    }
}

impl From<RuntimeProviderUsageRow> for RuntimeProviderUsageSummary {
    fn from(row: RuntimeProviderUsageRow) -> Self {
        Self {
            id: row.id,
            provider: row.provider,
            model: row.model,
            credential_scope: row.credential_scope,
            request_ms: row.request_ms,
            ttft_ms: row.ttft_ms,
            stream_ms: row.stream_ms,
            total_ms: row.total_ms,
            timeout_ms: row.timeout_ms,
            status: row.status,
            error_kind: row.error_kind,
            fallback_reason: row.fallback_reason,
            created_at: row.created_at,
        }
    }
}

async fn load_owned_credential_secret(
    state: &AppState,
    account_id: Uuid,
    id: Uuid,
) -> Result<CredentialSecretRow, (StatusCode, Json<Value>)> {
    let row = sqlx::query_as::<_, CredentialSecretRow>(
        "SELECT id, provider, org_id, account_id, secret_ciphertext, secret_nonce
           FROM runtime_provider_credentials
          WHERE id = $1
            AND status <> 'revoked'
            AND (
                account_id = $2
                OR org_id IN (SELECT org_id FROM org_members WHERE account_id = $2)
                OR scope = 'airnote_managed'
            )",
    )
    .bind(id)
    .bind(account_id)
    .fetch_optional(&state.db)
    .await
    .map_err(db_err)?
    .ok_or_else(|| {
        json_error(
            StatusCode::NOT_FOUND,
            "provider credential not found or not accessible",
        )
    })?;
    Ok(row)
}

#[derive(Clone)]
struct RuntimeProviderSecret {
    credential_id: Option<Uuid>,
    scope: String,
    secret: String,
}

async fn runtime_provider_secret(
    state: &AppState,
    account_id: Uuid,
    active_org_id: Option<Uuid>,
    provider: &str,
) -> Result<RuntimeProviderSecret, (StatusCode, Json<Value>)> {
    let row = if let Some(org_id) = active_org_id {
        sqlx::query_as::<_, CredentialSecretWithScopeRow>(
            "SELECT id, scope, secret_ciphertext, secret_nonce
               FROM runtime_provider_credentials
              WHERE provider = $2
                AND status = 'active'
                AND (
                    account_id = $1
                    OR org_id = $3
                    OR scope = 'airnote_managed'
                )
              ORDER BY
                CASE
                    WHEN account_id = $1 THEN 0
                    WHEN org_id = $3 THEN 1
                    WHEN scope = 'airnote_managed' THEN 2
                    ELSE 3
                END,
                updated_at DESC
              LIMIT 1",
        )
        .bind(account_id)
        .bind(provider)
        .bind(org_id)
        .fetch_optional(&state.db)
        .await
        .map_err(db_err)?
    } else {
        sqlx::query_as::<_, CredentialSecretWithScopeRow>(
            "SELECT id, scope, secret_ciphertext, secret_nonce
               FROM runtime_provider_credentials
              WHERE provider = $2
                AND status = 'active'
                AND (
                    account_id = $1
                    OR scope = 'airnote_managed'
                )
              ORDER BY
                CASE
                    WHEN account_id = $1 THEN 0
                    WHEN scope = 'airnote_managed' THEN 1
                    ELSE 2
                END,
                updated_at DESC
              LIMIT 1",
        )
        .bind(account_id)
        .bind(provider)
        .fetch_optional(&state.db)
        .await
        .map_err(db_err)?
    };

    let env_fallback_present = match provider {
        "deepgram" => !state.deepgram_api_key.trim().is_empty(),
        "openai" => !state.openai_api_key.trim().is_empty(),
        "groq" => !state.groq_api_key.trim().is_empty(),
        _ => false,
    };

    if let Some(row) = row {
        let secret = decrypt_secret(state, &row.secret_ciphertext, &row.secret_nonce)?;
        tracing::info!(
            "[runtime] credential resolved provider={} account_id={} vault_row=true env_fallback_present={} selected_scope={}",
            provider,
            account_id,
            env_fallback_present,
            row.scope,
        );
        return Ok(RuntimeProviderSecret {
            credential_id: Some(row.id),
            scope: row.scope,
            secret,
        });
    }

    let fallback = match provider {
        "deepgram" => state.deepgram_api_key.trim(),
        "openai" => state.openai_api_key.trim(),
        "groq" => state.groq_api_key.trim(),
        _ => "",
    };
    if tenant::allow_platform_credential_fallback() && !fallback.is_empty() {
        tracing::info!(
            "[runtime] credential resolved provider={} account_id={} vault_row=false env_fallback_present=true selected_scope=airnote_env",
            provider,
            account_id,
        );
        return Ok(RuntimeProviderSecret {
            credential_id: None,
            scope: "airnote_env".to_string(),
            secret: fallback.to_string(),
        });
    }

    tracing::warn!(
        "[runtime] credential missing provider={} account_id={} vault_row=false env_fallback_present=false",
        provider,
        account_id,
    );
    Err(json_error(
        StatusCode::SERVICE_UNAVAILABLE,
        &format!("{provider} provider credential is not configured"),
    ))
}

async fn update_credential_used(
    state: &AppState,
    credential_id: Option<Uuid>,
) -> Result<(), (StatusCode, Json<Value>)> {
    let Some(id) = credential_id else {
        return Ok(());
    };
    sqlx::query(
        "UPDATE runtime_provider_credentials
            SET last_used_at = now(), updated_at = now()
          WHERE id = $1",
    )
    .bind(id)
    .execute(&state.db)
    .await
    .map_err(db_err)?;
    Ok(())
}

async fn insert_provider_usage(
    state: &AppState,
    run_id: Uuid,
    credential: &RuntimeProviderSecret,
    provider: &str,
    model: Option<&str>,
    total_ms: Option<i64>,
    status: &str,
    error_kind: Option<&str>,
) -> Result<(), (StatusCode, Json<Value>)> {
    sqlx::query(
        "INSERT INTO runtime_provider_usage
            (credential_id, run_id, credential_scope, provider, model, total_ms, status, error_kind)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(credential.credential_id)
    .bind(run_id)
    .bind(&credential.scope)
    .bind(provider)
    .bind(model)
    .bind(total_ms)
    .bind(status)
    .bind(error_kind)
    .execute(&state.db)
    .await
    .map_err(db_err)?;
    Ok(())
}

fn content_hash(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

// ── Prompt/model helpers from transcript probe ──────────────────────────────

fn default_output_language() -> String {
    "hinglish".to_string()
}

fn default_selected_model() -> String {
    "smart".to_string()
}

fn default_voice_wav_mode() -> String {
    "normal_voice".to_string()
}

fn is_message_polish_wav_mode(mode: &str) -> bool {
    matches!(
        mode.trim().to_ascii_lowercase().as_str(),
        "message_polish" | "message-polish" | "polish_message" | "polish-my-message"
    )
}

fn default_runtime_mode() -> String {
    "normal_voice".to_string()
}

fn default_runtime_source() -> String {
    "desktop_voice".to_string()
}

#[derive(Debug)]
struct ProviderValidationError {
    status: StatusCode,
    message: String,
    permanent: bool,
}

impl ProviderValidationError {
    fn invalid(provider: &str) -> Self {
        let name = provider_display_name(provider);
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            message: format!(
                "{name} API key was rejected. Please paste a valid key and try again."
            ),
            permanent: true,
        }
    }

    fn unavailable(provider: &str, reason: impl Into<String>) -> Self {
        let name = provider_display_name(provider);
        let reason = reason.into();
        Self {
            status: StatusCode::BAD_GATEWAY,
            message: format!("Could not validate {name} API key right now: {reason}"),
            permanent: false,
        }
    }

    fn into_response(self) -> (StatusCode, Json<Value>) {
        json_error(self.status, &self.message)
    }
}

fn provider_display_name(provider: &str) -> &'static str {
    match provider {
        "deepgram" => "Deepgram",
        "groq" => "Groq",
        "openai" => "OpenAI",
        "gemini" => "Gemini",
        "gateway" => "Gateway",
        _ => "Provider",
    }
}

async fn validate_provider_secret(
    provider: &str,
    secret: &str,
) -> Result<(), ProviderValidationError> {
    let client = &*crate::HTTP_CLIENT;
    let timeout = Duration::from_secs(10);
    let resp = match provider {
        "deepgram" => {
            client
                .get(DEEPGRAM_VALIDATE_ENDPOINT)
                .header("Authorization", format!("Token {secret}"))
                .timeout(timeout)
                .send()
                .await
        }
        "groq" => {
            client
                .get(GROQ_VALIDATE_ENDPOINT)
                .bearer_auth(secret)
                .timeout(timeout)
                .send()
                .await
        }
        "openai" => {
            client
                .get(OPENAI_VALIDATE_ENDPOINT)
                .bearer_auth(secret)
                .timeout(timeout)
                .send()
                .await
        }
        "gemini" => {
            let url = format!(
                "{GEMINI_VALIDATE_ENDPOINT}?key={}",
                urlencoding::encode(secret)
            );
            client.get(url).timeout(timeout).send().await
        }
        "gateway" => {
            let body = json!({
                "model": GROQ_MODEL_FAST,
                "stream": false,
                "max_tokens": 1,
                "temperature": 0,
                "messages": [
                    { "role": "user", "content": "ping" }
                ]
            });
            client
                .post(GATEWAY_VALIDATE_ENDPOINT)
                .header("X-API-Key", secret)
                .header("Content-Type", "application/json")
                .json(&body)
                .timeout(timeout)
                .send()
                .await
        }
        _ => return Err(ProviderValidationError::invalid(provider)),
    }
    .map_err(|e| {
        let reason = if e.is_timeout() {
            "provider validation timed out"
        } else {
            "provider validation request failed"
        };
        ProviderValidationError::unavailable(provider, reason)
    })?;

    let status = resp.status();
    if status.is_success() || status.as_u16() == 429 {
        return Ok(());
    }

    if status.as_u16() == 401
        || status.as_u16() == 403
        || (provider == "gemini" && status.as_u16() == 400)
    {
        return Err(ProviderValidationError::invalid(provider));
    }

    Err(ProviderValidationError::unavailable(
        provider,
        format!("provider returned HTTP {status}"),
    ))
}

fn normalize_provider(provider: &str) -> Result<String, (StatusCode, Json<Value>)> {
    let provider = provider.trim().to_lowercase();
    match provider.as_str() {
        "deepgram" | "groq" | "openai" | "gemini" | "gateway" => Ok(provider),
        _ => Err(json_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "unknown provider",
        )),
    }
}

fn normalize_scope(scope: Option<&str>) -> Result<String, (StatusCode, Json<Value>)> {
    let scope = scope.unwrap_or("user").trim().to_lowercase();
    match scope.as_str() {
        "user" | "org" | "airnote_managed" => Ok(scope),
        _ => Err(json_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "unknown scope",
        )),
    }
}

fn restore_literal_tokens(transcript: &str, output: &str, safe_vocab_terms: &[String]) -> String {
    let source_words = transcript.split_whitespace().collect::<Vec<_>>();
    let mut output_words = output
        .split_whitespace()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if source_words.is_empty() || source_words.len() != output_words.len() {
        return output.to_string();
    }

    let mut changed = false;
    for (source, out_word) in source_words.iter().zip(output_words.iter_mut()) {
        let source_core = trim_token_edges(source);
        let output_core = trim_token_edges(out_word);
        if source_core.is_empty() || output_core.is_empty() {
            continue;
        }
        if !is_literal_preserve_token(source_core, safe_vocab_terms) {
            continue;
        }
        if contains_token_case_insensitive(output, source_core) {
            continue;
        }
        if !source_core.eq_ignore_ascii_case(output_core) {
            *out_word = replace_token_core(out_word, source_core);
            changed = true;
        }
    }

    if changed {
        output_words.join(" ")
    } else {
        output.to_string()
    }
}

fn restore_numeric_literal_tokens(transcript: &str, output: &str) -> String {
    let source_words = transcript.split_whitespace().collect::<Vec<_>>();
    let mut output_words = output
        .split_whitespace()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if source_words.is_empty() || source_words.len() != output_words.len() {
        return output.to_string();
    }

    let mut changed = false;
    for (source, out_word) in source_words.iter().zip(output_words.iter_mut()) {
        let Some(source_core) = numeric_literal_core(source) else {
            continue;
        };
        let Some(output_core) = numeric_literal_core(out_word) else {
            continue;
        };
        if source_core == output_core {
            continue;
        }
        if numeric_digits(&source_core) != numeric_digits(&output_core) {
            continue;
        }
        *out_word = replace_numeric_token_core(out_word, &source_core);
        changed = true;
    }

    if changed {
        output_words.join(" ")
    } else {
        output.to_string()
    }
}

fn numeric_literal_core(token: &str) -> Option<String> {
    let core = token
        .trim_matches(|c: char| !(c.is_ascii_digit() || matches!(c, '$' | '₹' | '%' | '.' | ',')));
    if core.is_empty() || !core.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    let has_format_symbol = core
        .chars()
        .any(|c| matches!(c, '$' | '₹' | '%' | '.' | ','));
    if has_format_symbol {
        Some(core.to_string())
    } else {
        None
    }
}

fn numeric_digits(token: &str) -> String {
    token.chars().filter(|c| c.is_ascii_digit()).collect()
}

fn replace_numeric_token_core(output_word: &str, source_core: &str) -> String {
    let start = output_word
        .char_indices()
        .find(|(_, c)| c.is_ascii_digit() || matches!(c, '$' | '₹' | '%' | '.' | ','))
        .map(|(idx, _)| idx)
        .unwrap_or(0);
    let end = output_word
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_ascii_digit() || matches!(c, '$' | '₹' | '%' | '.' | ','))
        .map(|(idx, c)| idx + c.len_utf8())
        .unwrap_or(output_word.len());

    format!(
        "{}{}{}",
        &output_word[..start],
        source_core,
        &output_word[end..]
    )
}

fn trim_token_edges(token: &str) -> &str {
    token.trim_matches(|c: char| !c.is_alphanumeric() && c != '_' && c != '-' && c != '.')
}

fn is_literal_preserve_token(token: &str, safe_vocab_terms: &[String]) -> bool {
    if safe_vocab_terms
        .iter()
        .any(|term| term.trim().eq_ignore_ascii_case(token))
    {
        return true;
    }
    let has_upper = token.chars().any(|c| c.is_ascii_uppercase());
    let has_internal_upper = token.chars().skip(1).any(|c| c.is_ascii_uppercase());
    let has_digit_or_symbol = token
        .chars()
        .any(|c| c.is_ascii_digit() || matches!(c, '_' | '-' | '.' | '@' | '/'));
    let is_all_caps = token
        .chars()
        .all(|c| !c.is_ascii_alphabetic() || c.is_ascii_uppercase())
        && token.chars().any(|c| c.is_ascii_alphabetic());

    token.len() >= 3 && (has_digit_or_symbol || has_internal_upper || is_all_caps || has_upper)
}

fn contains_token_case_insensitive(text: &str, token: &str) -> bool {
    text.split_whitespace()
        .map(trim_token_edges)
        .any(|part| part.eq_ignore_ascii_case(token))
}

fn replace_token_core(output_word: &str, source_core: &str) -> String {
    let start = output_word
        .char_indices()
        .find(|(_, c)| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))
        .map(|(idx, _)| idx)
        .unwrap_or(0);
    let end = output_word
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))
        .map(|(idx, c)| idx + c.len_utf8())
        .unwrap_or(output_word.len());

    format!(
        "{}{}{}",
        &output_word[..start],
        source_core,
        &output_word[end..]
    )
}

/// Codex (ChatGPT) model used for polish when an org has connected ChatGPT.
const CODEX_POLISH_MODEL: &str = "gpt-5.4-mini";

/// Deterministic sentence-case + terminal punctuation for the dictation output.
/// The light-touch polish prompt under-edits casing/punctuation; this guarantees
/// them mechanically. Conservative: only capitalizes the first letter of each
/// sentence and appends a single '.' when the text ends without terminal
/// punctuation. Never rewrites words, so it can't re-trigger LLM over-editing.
fn tidy_casing(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return input.to_string();
    }
    let mut out = String::with_capacity(trimmed.len() + 1);
    let mut at_sentence_start = true;
    for ch in trimmed.chars() {
        if at_sentence_start && ch.is_alphabetic() {
            out.extend(ch.to_uppercase());
            at_sentence_start = false;
            continue;
        }
        out.push(ch);
        if ch == '.' || ch == '?' || ch == '!' {
            at_sentence_start = true;
        } else if !ch.is_whitespace() {
            at_sentence_start = false;
        }
    }
    if out
        .chars()
        .rev()
        .find(|c| !c.is_whitespace())
        .is_some_and(char::is_alphanumeric)
    {
        out.push('.');
    }
    out
}

#[cfg(test)]
mod tidy_casing_tests {
    use super::tidy_casing;

    #[test]
    fn capitalizes_and_terminates() {
        assert_eq!(tidy_casing("haan ye theek hai"), "Haan ye theek hai.");
        assert_eq!(tidy_casing("yes. okay"), "Yes. Okay.");
    }

    #[test]
    fn leaves_correct_text_unchanged() {
        assert_eq!(tidy_casing("Hello world."), "Hello world.");
        assert_eq!(tidy_casing("Is it ready?"), "Is it ready?");
    }

    #[test]
    fn empty_and_numeric_safe() {
        assert_eq!(tidy_casing(""), "");
        assert_eq!(tidy_casing("250 ms"), "250 ms.");
    }
}

/// The org's connected-ChatGPT access token, transparently refreshed if expired.
/// Returns `None` when the org hasn't connected ChatGPT (so polish stays on Groq,
/// byte-for-byte unchanged) or when a refresh fails.
async fn active_openai_token(state: &AppState, org_id: Uuid) -> Option<String> {
    let row: Option<(
        Option<String>,
        Option<String>,
        Option<chrono::DateTime<chrono::Utc>>,
    )> = sqlx::query_as(
        "SELECT openai_access_token, openai_refresh_token, openai_token_expires_at \
         FROM orgs WHERE id = $1",
    )
    .bind(org_id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();
    let (access, refresh, expires) = row?;
    let access = access.filter(|t| !t.trim().is_empty())?;

    // Still valid (60s safety margin), or no expiry recorded → use as-is.
    let needs_refresh =
        matches!(expires, Some(exp) if exp <= chrono::Utc::now() + chrono::Duration::seconds(60));
    if !needs_refresh {
        return Some(access);
    }

    // Expired → refresh via the codex client and persist the rotated tokens.
    let refresh = refresh.filter(|t| !t.trim().is_empty())?;
    let tokens = crate::codex_client::refresh_token(&refresh).await.ok()?;
    let new_refresh = tokens.refresh_token.clone().unwrap_or(refresh);
    let new_expires = chrono::Utc::now() + chrono::Duration::seconds(tokens.expires_in);
    let _ = sqlx::query(
        "UPDATE orgs SET openai_access_token = $1, openai_refresh_token = $2, \
         openai_token_expires_at = $3 WHERE id = $4",
    )
    .bind(&tokens.access_token)
    .bind(&new_refresh)
    .bind(new_expires)
    .bind(org_id)
    .execute(&state.db)
    .await;
    Some(tokens.access_token)
}

/// Polish via the org's connected ChatGPT (Codex) when available, else Groq.
///
/// ANY Codex problem — no connection, expired/invalid token, API error, timeout,
/// or empty output — silently falls back to Groq, so dictation can never break.
/// Orgs that haven't connected ChatGPT take the Groq path with zero behaviour
/// change. This mirrors the desktop's "ChatGPT polishes your dictation, falls back
/// to Groq" model, at the cloud/org level. Desktop is unaffected (it polishes
/// locally and never calls this endpoint).
async fn polish_llm(
    state: &AppState,
    org_id: Option<Uuid>,
    groq_secret: &str,
    groq_model: &str,
    system_prompt: &str,
    user_message: &str,
) -> Result<String, (StatusCode, Json<Value>)> {
    if let Some(org_id) = org_id {
        if let Some(token) = active_openai_token(state, org_id).await {
            let codex = tokio::time::timeout(
                std::time::Duration::from_secs(15),
                crate::codex_client::call_codex(
                    &token,
                    CODEX_POLISH_MODEL,
                    system_prompt,
                    user_message,
                ),
            )
            .await;
            match codex {
                Ok(Ok(resp)) if !resp.text.trim().is_empty() => return Ok(resp.text),
                Ok(Ok(_)) => tracing::warn!("[polish] codex returned empty — falling back to groq"),
                Ok(Err(e)) => {
                    tracing::warn!("[polish] codex failed ({e}) — falling back to groq")
                }
                Err(_) => tracing::warn!("[polish] codex timed out — falling back to groq"),
            }
        }
    }
    call_groq(state, groq_secret, groq_model, system_prompt, user_message).await
}

async fn call_groq(
    _state: &AppState,
    api_key: &str,
    model: &str,
    system_prompt: &str,
    user_message: &str,
) -> Result<String, (StatusCode, Json<Value>)> {
    let estimated_input_tokens = user_message.len() / 4;
    let max_tokens = (estimated_input_tokens * 2 + 256).min(4096);
    let body = json!({
        "model": model,
        // 0.2, not greedy 0.0. Groq clamps temperature 0 to 1e-8 — effectively
        // greedy — which is the trigger for Llama-4-Scout repetition meltdowns
        // ("The The The…") on long Hinglish input. Groq silently ignores
        // frequency/presence penalties, so a small temperature + a short prompt
        // is the only working mitigation (Holtzman 2019; temp-0 48x-loop study).
        "temperature": 0.2,
        "top_p": 0.9,
        "max_tokens": max_tokens,
        "stream": false,
        "stop": [
            "=== BEGIN TRANSCRIPT",
            "=== END TRANSCRIPT",
            "<transcript>",
            "</transcript>"
        ],
        "messages": [
            { "role": "system", "content": system_prompt },
            { "role": "user", "content": user_message }
        ]
    });

    let client = &*crate::HTTP_CLIENT;
    let resp = client
        .post(GROQ_ENDPOINT)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await
        .map_err(|e| {
            json_error(
                StatusCode::BAD_GATEWAY,
                &format!("server runtime model request failed: {e}"),
            )
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let preview = resp.text().await.unwrap_or_default();
        tracing::warn!(
            "[runtime] Groq HTTP {status}: {}",
            &preview[..preview.len().min(300)]
        );
        return Err(json_error(
            StatusCode::BAD_GATEWAY,
            &format!("Groq returned {status}"),
        ));
    }

    let value: Value = resp.json().await.map_err(|e| {
        json_error(
            StatusCode::BAD_GATEWAY,
            &format!("server runtime model response parse failed: {e}"),
        )
    })?;

    let output = value
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    if output.is_empty() {
        return Err(json_error(
            StatusCode::BAD_GATEWAY,
            "server runtime model returned empty output",
        ));
    }

    Ok(output)
}

// ── Learning memory: helpers, loader, resolver ─────────────────────────────

struct RuntimeMemory {
    vocab_terms: Vec<String>,
    replacements: Vec<(String, String)>,
    policy_rules: Vec<(String, String)>,
}

impl Default for RuntimeMemory {
    fn default() -> Self {
        Self {
            vocab_terms: Vec::new(),
            replacements: Vec::new(),
            policy_rules: Vec::new(),
        }
    }
}

fn normalize_learning_text(text: &str) -> String {
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

fn is_common_learning_term(norm: &str) -> bool {
    const COMMON: &[&str] = &[
        "kaisa",
        "kaisi",
        "kaise",
        "aisa",
        "aisi",
        "aise",
        "laga",
        "lagi",
        "lage",
        "main",
        "mein",
        "hai",
        "hain",
        "tha",
        "thi",
        "the",
        "time",
        "can",
        "go",
        "do",
        "this",
        "for",
        "me",
        "one thing",
        "ek baar",
        "char log",
        "kaam",
        "kya",
        "kyun",
        "aur",
        "batao",
        "bolo",
        "karo",
        "karna",
        "kar",
        "bhejo",
        "dikhao",
        "kholo",
        "a",
        "an",
        "is",
        "are",
        "was",
        "were",
        "be",
        "been",
        "being",
        "have",
        "has",
        "had",
        "does",
        "did",
        "will",
        "would",
        "could",
        "should",
        "may",
        "might",
        "must",
        "shall",
        "and",
        "or",
        "but",
        "if",
        "in",
        "on",
        "at",
        "to",
        "of",
        "it",
        "its",
        "that",
        "which",
        "who",
        "not",
        "no",
        "yes",
        "ok",
        "okay",
        "yeah",
        "yep",
        "nope",
        "open",
        "close",
        "send",
        "return",
        "source",
        "schema",
        "resolver",
        "chart",
        "bank",
        "smallcap",
        "small",
        "cap",
        "one",
        "two",
        "too",
        "three",
        "four",
        "five",
        "six",
        "seven",
        "eight",
        "nine",
        "ten",
        "ek",
        "do",
        "teen",
        "char",
        "panch",
        "paanch",
        "ka",
        "ke",
        "ki",
        "ko",
        "se",
        "par",
        "pe",
    ];
    if COMMON.contains(&norm) {
        return true;
    }
    let tokens: Vec<_> = norm.split_whitespace().collect();
    !tokens.is_empty() && tokens.iter().all(|t| COMMON.contains(t))
}

fn is_allowed_term_type(term_type: &str) -> bool {
    matches!(
        term_type,
        "brand" | "acronym" | "code_identifier" | "proper_noun"
    )
}

fn count_words(text: &str) -> usize {
    text.split_whitespace().count()
}

async fn learning_judge_candidates(
    state: &AppState,
    account_id: Uuid,
    req: &AnalyzeEditLearningRequest,
    edit_spans: &[UserEditSpan],
) -> Result<Vec<LearningReviewCandidate>, (StatusCode, Json<Value>)> {
    let active_org_id = primary_org_id(state, account_id).await?;
    let credential = runtime_provider_secret(state, account_id, active_org_id, "groq").await?;
    let system_prompt = r#"You are AirNote's edit-learning judge.

You receive:
- raw_transcript: what Deepgram actually produced
- pasted_output: what AirNote pasted
- user_kept: what the user edited it to
- exact_user_edit_spans: deterministic pasted_output -> user_kept spans

Return ONLY strict JSON with key "candidates".
Each candidate is a possible STT replacement for user review, not an automatic save.

Rules:
- First authority is exact_user_edit_spans. If a candidate comes from a user edit, its corrected text MUST come from kept_span and its source should come from pasted_span unless pasted_span is empty.
- Do not choose a different target word than the kept_span the user actually typed.
- Do not choose a source word outside exact_user_edit_spans unless pasted_span is empty and raw_transcript clearly contains the missing STT source at the same position.
- Mine alias sources from raw_transcript only as supporting evidence, not as a replacement for exact_user_edit_spans.
- Do NOT mine aliases from text introduced only by pasted_output or user_kept.
- If pasted_output changed a raw term and user reverted to raw, output no candidate; that is polish negative feedback, not STT learning.
- If the edit is email, URL, number formatting, casing, punctuation, grammar, translation, or full rewrite, output no STT candidates.
- Never use common/filler words as source: main, mein, kaisa, laga, hai, ka, ke, ki, data, time, can, go, etc.
- Preserve the full contiguous raw phonetic/spelled source span when it maps to one protected target.
- Do NOT drop meaningful phonetic prefix/split tokens such as: super, post, graph, cute, same, rush, hub, spot, mail, chimp, a h, g a, n eight n, next j s, info sis.
- Trim ONLY true filler/function edge tokens from this exact list: main, mein, mai, ka, ke, ki, ko, se, par, pe, hai, ho, karo, karna, bhejo, batao.
- If the corrected text is only a number, common action word, casing change, or acronym casing of the same raw word, output no candidate.
- Target must be a protected-looking brand, acronym, proper noun, or code identifier.
- When unsure, output no candidates.

Examples:
- raw "super base mein auth" kept "Supabase mein auth" -> {"original":"super base","corrected":"Supabase","term_type":"brand","learnable":true,"tag":"server_llm"}
- raw "post grass index" kept "Postgres index" -> {"original":"post grass","corrected":"Postgres","term_type":"brand","learnable":true,"tag":"server_llm"}
- raw "a h refs report" kept "Ahrefs report" -> {"original":"a h refs","corrected":"Ahrefs","term_type":"brand","learnable":true,"tag":"server_llm"}
- raw "same rush keywords" kept "Semrush keywords" -> {"original":"same rush","corrected":"Semrush","term_type":"brand","learnable":true,"tag":"server_llm"}
- raw "n eight n workflow" kept "n8n workflow" -> {"original":"n eight n","corrected":"n8n","term_type":"code_identifier","learnable":true,"tag":"server_llm"}
- raw "g a four events" kept "GA4 events" -> {"original":"g a four","corrected":"GA4","term_type":"code_identifier","learnable":true,"tag":"server_llm"}
- raw "hello main Gops ka" kept "hello Macobs ka" -> {"original":"Gops","corrected":"Macobs","term_type":"brand","learnable":true,"tag":"server_llm"}
- pasted "Emeic ka update" kept "Mac ka update" -> {"original":"Emeic","corrected":"Mac","term_type":"brand","learnable":true,"tag":"server_llm"}
- pasted "Emeic ka update" kept "Mac ka update" must NEVER return corrected "EMIAC" because the user did not type EMIAC.
- raw "cursor open karo" kept "Cursor open karo" -> no candidates
- raw "roas 3.2 hai" kept "ROAS 3.2 hai" -> no candidates

Candidate shape:
{"original": "raw source span", "corrected": "target from user_kept", "term_type": "brand|acronym|code_identifier|proper_noun", "learnable": true, "tag": "server_llm"}
"#;
    let user_message = format!(
        r#"raw_transcript:
{}

pasted_output:
{}

user_kept:
{}

exact_user_edit_spans_json:
{}

Return JSON only:
{{"candidates":[...]}}"#,
        req.transcript,
        req.ai_output,
        req.user_kept,
        serde_json::to_string(edit_spans).unwrap_or_else(|_| "[]".to_string())
    );
    let raw = call_groq(
        state,
        &credential.secret,
        &learning_judge_model(),
        system_prompt,
        &user_message,
    )
    .await?;
    let Some(value) = parse_json_object_from_model(&raw) else {
        tracing::warn!("[runtime] learning judge returned invalid JSON");
        return Ok(Vec::new());
    };
    let candidates = value
        .get("candidates")
        .cloned()
        .unwrap_or_else(|| json!([]));
    match serde_json::from_value::<Vec<LearningReviewCandidate>>(candidates) {
        Ok(candidates) => Ok(candidates),
        Err(e) => {
            tracing::warn!("[runtime] learning judge candidates parse failed: {e}");
            Ok(Vec::new())
        }
    }
}

async fn validate_learning_candidates_with_judge(
    state: &AppState,
    account_id: Uuid,
    req: &AnalyzeEditLearningRequest,
) -> Result<Vec<LearningReviewCandidate>, (StatusCode, Json<Value>)> {
    let active_org_id = primary_org_id(state, account_id).await?;
    let credential = runtime_provider_secret(state, account_id, active_org_id, "groq").await?;
    let system_prompt = r#"You are AirNote's edit-learning validation judge.

You receive:
- raw_transcript: what Deepgram actually produced
- pasted_output: what AirNote pasted
- user_kept: what the user edited it to
- proposed_candidates: local desktop diff candidates

Your task:
Approve only candidates that are safe STT-learning replacements.
Return ONLY strict JSON with key "candidates".

Validation rules:
- The source/original must be a real span from raw_transcript or its romanized/pasted surface.
- The source must NOT be a common Hindi/Hinglish/English word or filler phrase.
- The source can be gibberish/phonetic ASR output like "myak", "Gops", "mecobs", "n s e", "a h refs".
- The corrected form must appear in user_kept and must be a protected-looking brand, acronym, proper noun, or code identifier.
- If the proposed original includes filler/function edge tokens, trim them. Example: "main Gops" -> "Gops"; "n s e ka" -> "n s e".
- If this is only casing, grammar, punctuation, email/URL formatting, number formatting, translation, or a full rewrite, return no candidates.
- If the user reverted a model over-correction, return no candidates.
- When unsure, reject by returning no candidate.

Examples:
- proposed "myak" -> "EMIAC", raw has "myak", kept has "EMIAC"/"Emiac" -> approve.
- proposed "Gops" -> "Macobs", raw/pasted has "Gops", kept has "Macobs" -> approve.
- proposed "main Gops" -> "Macobs" -> approve as "Gops" -> "Macobs".
- proposed "kaisa" -> "Macobs" -> reject.
- proposed "cursor" -> "Cursor" -> reject.
- proposed "n s e ka" -> "NSE" -> approve as "n s e" -> "NSE".

Candidate shape:
{"original": "approved source span", "corrected": "approved corrected form", "term_type": "brand|acronym|code_identifier|proper_noun", "learnable": true, "tag": "server_llm_candidate_validation"}
"#;
    let user_message = format!(
        r#"raw_transcript:
{}

pasted_output:
{}

user_kept:
{}

proposed_candidates:
{}

Return JSON only:
{{"candidates":[...]}}"#,
        req.transcript,
        req.ai_output,
        req.user_kept,
        serde_json::to_string(&req.candidates).unwrap_or_else(|_| "[]".to_string())
    );
    let raw = call_groq(
        state,
        &credential.secret,
        &learning_judge_model(),
        system_prompt,
        &user_message,
    )
    .await?;
    let Some(value) = parse_json_object_from_model(&raw) else {
        tracing::warn!("[runtime] learning candidate validator returned invalid JSON");
        return Ok(Vec::new());
    };
    let candidates = value
        .get("candidates")
        .cloned()
        .unwrap_or_else(|| json!([]));
    match serde_json::from_value::<Vec<LearningReviewCandidate>>(candidates) {
        Ok(candidates) => Ok(candidates),
        Err(e) => {
            tracing::warn!("[runtime] learning candidate validator parse failed: {e}");
            Ok(Vec::new())
        }
    }
}

fn parse_json_object_from_model(raw: &str) -> Option<Value> {
    let trimmed = raw.trim();
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        return Some(value);
    }
    let start = trimmed.find('{')?;
    let end = trimmed.rfind('}')?;
    if end <= start {
        return None;
    }
    serde_json::from_str::<Value>(&trimmed[start..=end]).ok()
}

fn looks_like_formatter_only_memory(text: &str) -> bool {
    let lower = text.trim().to_lowercase();
    if lower.is_empty() {
        return false;
    }
    if looks_like_numeric_format_memory(&lower) {
        return true;
    }
    if (lower.contains('@') && lower.contains('.'))
        || lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.contains("www.")
    {
        return true;
    }

    let norm = normalize_learning_text(&lower);
    let tokens: Vec<&str> = norm.split_whitespace().collect();
    let has_email_domain = tokens.iter().any(|token| {
        matches!(
            *token,
            "gmail" | "outlook" | "hotmail" | "yahoo" | "icloud" | "proton" | "zoho"
        )
    }) || lower.contains("gmail.com")
        || lower.contains("outlook.com")
        || lower.contains("hotmail.com")
        || lower.contains("yahoo.com")
        || lower.contains("icloud.com");
    let has_email_operator =
        tokens.iter().any(|token| matches!(*token, "at" | "dot")) || lower.contains("at the rate");

    has_email_domain && has_email_operator
}

fn looks_like_numeric_format_memory(text: &str) -> bool {
    let has_digit = text.chars().any(|c| c.is_ascii_digit());
    if !has_digit {
        return false;
    }
    let norm = normalize_learning_text(text);
    let tokens = norm.split_whitespace().collect::<Vec<_>>();
    if tokens.is_empty() {
        return false;
    }
    tokens.iter().all(|token| {
        token.chars().all(|c| c.is_ascii_digit())
            || matches!(
                *token,
                "percent"
                    | "percentage"
                    | "gb"
                    | "mb"
                    | "tb"
                    | "kb"
                    | "usd"
                    | "inr"
                    | "rs"
                    | "rupee"
                    | "rupees"
                    | "rupaye"
                    | "dollar"
                    | "dollars"
                    | "hundred"
                    | "thousand"
                    | "lakh"
                    | "crore"
                    | "million"
                    | "billion"
                    | "jan"
                    | "january"
                    | "feb"
                    | "february"
                    | "mar"
                    | "march"
                    | "apr"
                    | "april"
                    | "may"
                    | "jun"
                    | "june"
                    | "jul"
                    | "july"
                    | "aug"
                    | "august"
                    | "sep"
                    | "sept"
                    | "september"
                    | "oct"
                    | "october"
                    | "nov"
                    | "november"
                    | "dec"
                    | "december"
            )
    })
}

fn compact_learning_text(text: &str) -> String {
    normalize_learning_text(text)
        .chars()
        .filter(|c| !c.is_whitespace() && !matches!(c, '.' | '-' | '_'))
        .collect()
}

fn normalized_text_contains_span(text: &str, span: &str) -> bool {
    let target_tokens = normalized_tokens(span)
        .into_iter()
        .map(|(_, norm)| norm)
        .collect::<Vec<_>>();
    if target_tokens.is_empty() {
        return false;
    }
    let haystack_tokens = normalized_tokens(text)
        .into_iter()
        .map(|(_, norm)| norm)
        .collect::<Vec<_>>();
    haystack_tokens
        .windows(target_tokens.len())
        .any(|window| window == target_tokens.as_slice())
}

#[derive(Debug)]
enum UserEditOp {
    Equal,
    Delete(usize),
    Insert(usize),
}

/// Deterministically extract exact pasted_output -> user_kept word spans.
/// This function does not decide learnability; it only records what changed.
fn extract_user_edit_spans(pasted_output: &str, user_kept: &str) -> Vec<UserEditSpan> {
    let pasted = normalized_tokens(pasted_output);
    let kept = normalized_tokens(user_kept);
    if pasted
        .iter()
        .map(|(_, norm)| norm)
        .eq(kept.iter().map(|(_, norm)| norm))
    {
        return Vec::new();
    }

    let n = pasted.len();
    let m = kept.len();
    let mut lcs = vec![vec![0usize; m + 1]; n + 1];
    for i in 0..n {
        for j in 0..m {
            if pasted[i].1 == kept[j].1 {
                lcs[i + 1][j + 1] = lcs[i][j] + 1;
            } else {
                lcs[i + 1][j + 1] = lcs[i + 1][j].max(lcs[i][j + 1]);
            }
        }
    }

    let mut ops = Vec::new();
    let mut i = n;
    let mut j = m;
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && pasted[i - 1].1 == kept[j - 1].1 {
            ops.push(UserEditOp::Equal);
            i -= 1;
            j -= 1;
        } else if j > 0 && (i == 0 || lcs[i][j - 1] >= lcs[i - 1][j]) {
            ops.push(UserEditOp::Insert(j - 1));
            j -= 1;
        } else {
            ops.push(UserEditOp::Delete(i - 1));
            i -= 1;
        }
    }
    ops.reverse();

    let mut spans = Vec::new();
    let mut current_pasted = Vec::new();
    let mut current_kept = Vec::new();
    let mut pasted_start: Option<usize> = None;
    let mut kept_start: Option<usize> = None;
    let mut pasted_cursor = 0usize;
    let mut kept_cursor = 0usize;

    let flush = |spans: &mut Vec<UserEditSpan>,
                 current_pasted: &mut Vec<usize>,
                 current_kept: &mut Vec<usize>,
                 pasted_start: &mut Option<usize>,
                 kept_start: &mut Option<usize>,
                 pasted_cursor: usize,
                 kept_cursor: usize| {
        if current_pasted.is_empty() && current_kept.is_empty() {
            return;
        }
        let p_start = pasted_start.unwrap_or(pasted_cursor);
        let k_start = kept_start.unwrap_or(kept_cursor);
        let pasted_span = current_pasted
            .iter()
            .filter_map(|idx| pasted.get(*idx).map(|(surface, _)| surface.as_str()))
            .collect::<Vec<_>>()
            .join(" ");
        let kept_span = current_kept
            .iter()
            .filter_map(|idx| kept.get(*idx).map(|(surface, _)| surface.as_str()))
            .collect::<Vec<_>>()
            .join(" ");
        let left_context = kept[k_start.saturating_sub(3)..k_start]
            .iter()
            .map(|(surface, _)| surface.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let right_start = kept_cursor.min(kept.len());
        let right_end = (right_start + 3).min(kept.len());
        let right_context = kept[right_start..right_end]
            .iter()
            .map(|(surface, _)| surface.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        spans.push(UserEditSpan {
            pasted_span,
            kept_span,
            left_context,
            right_context,
            pasted_start: p_start,
            kept_start: k_start,
        });
        current_pasted.clear();
        current_kept.clear();
        *pasted_start = None;
        *kept_start = None;
    };

    for op in ops {
        match op {
            UserEditOp::Equal => {
                flush(
                    &mut spans,
                    &mut current_pasted,
                    &mut current_kept,
                    &mut pasted_start,
                    &mut kept_start,
                    pasted_cursor,
                    kept_cursor,
                );
                pasted_cursor += 1;
                kept_cursor += 1;
            }
            UserEditOp::Delete(idx) => {
                if pasted_start.is_none() {
                    pasted_start = Some(pasted_cursor);
                }
                if kept_start.is_none() {
                    kept_start = Some(kept_cursor);
                }
                current_pasted.push(idx);
                pasted_cursor += 1;
            }
            UserEditOp::Insert(idx) => {
                if pasted_start.is_none() {
                    pasted_start = Some(pasted_cursor);
                }
                if kept_start.is_none() {
                    kept_start = Some(kept_cursor);
                }
                current_kept.push(idx);
                kept_cursor += 1;
            }
        }
    }
    flush(
        &mut spans,
        &mut current_pasted,
        &mut current_kept,
        &mut pasted_start,
        &mut kept_start,
        pasted_cursor,
        kept_cursor,
    );

    spans
}

fn deterministic_user_edit_span_candidates(spans: &[UserEditSpan]) -> Vec<LearningReviewCandidate> {
    spans
        .iter()
        .filter_map(|span| {
            let corrected = span.kept_span.trim();
            if corrected.is_empty()
                || count_words(&normalize_learning_text(corrected)) > 4
                || !looks_like_protected_target(corrected)
            {
                return None;
            }
            let original = span.pasted_span.trim();
            if normalize_learning_text(original) == normalize_learning_text(corrected) {
                return None;
            }
            Some(LearningReviewCandidate {
                original: original.to_string(),
                corrected: corrected.to_string(),
                term_type: infer_term_type_from_target(corrected).to_string(),
                learnable: true,
                tag: "server_exact_user_edit_span".to_string(),
            })
        })
        .collect()
}

enum CandidateContextTrim {
    Unchanged,
    Drop,
    Trim(LearningReviewCandidate),
}

fn trim_unchanged_candidate_context(candidate: &LearningReviewCandidate) -> CandidateContextTrim {
    let original_tokens = normalized_tokens(&candidate.original);
    let corrected_tokens = normalized_tokens(&candidate.corrected);
    if original_tokens.len() <= 1 || original_tokens.len() != corrected_tokens.len() {
        return CandidateContextTrim::Unchanged;
    }

    let mut start = 0usize;
    while start < original_tokens.len() && original_tokens[start].1 == corrected_tokens[start].1 {
        start += 1;
    }

    let mut end = original_tokens.len();
    while end > start && original_tokens[end - 1].1 == corrected_tokens[end - 1].1 {
        end -= 1;
    }

    if start == 0 && end == original_tokens.len() {
        return CandidateContextTrim::Unchanged;
    }
    if start >= end {
        return CandidateContextTrim::Drop;
    }

    let original = original_tokens[start..end]
        .iter()
        .map(|(surface, _)| surface.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let corrected = corrected_tokens[start..end]
        .iter()
        .map(|(surface, _)| surface.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let corrected_norm = normalize_learning_text(&corrected);
    if corrected_norm.is_empty() || is_common_learning_term(&corrected_norm) {
        return CandidateContextTrim::Drop;
    }
    if !looks_like_protected_target(&corrected) {
        return CandidateContextTrim::Drop;
    }

    let mut trimmed = candidate.clone();
    trimmed.original = original;
    trimmed.corrected = corrected.clone();
    trimmed.term_type = infer_term_type_from_target(&corrected).to_string();
    if !trimmed.tag.contains("trimmed") {
        trimmed.tag = format!("{}_trimmed", trimmed.tag);
    }
    CandidateContextTrim::Trim(trimmed)
}

fn deterministic_user_edit_span_candidates_for_text(
    pasted_output: &str,
    user_kept: &str,
    spans: &[UserEditSpan],
) -> Vec<LearningReviewCandidate> {
    if token_overlap_ratio(pasted_output, user_kept) < 0.45 {
        return Vec::new();
    }
    deterministic_user_edit_span_candidates(spans)
}

fn deterministic_user_edit_span_candidates_for_request(
    req: &AnalyzeEditLearningRequest,
    spans: &[UserEditSpan],
) -> Vec<LearningReviewCandidate> {
    deterministic_user_edit_span_candidates_for_text(&req.ai_output, &req.user_kept, spans)
        .into_iter()
        .filter(|candidate| exact_span_candidate_survives_raw_context(req, candidate))
        .collect()
}

fn exact_span_candidate_survives_raw_context(
    req: &AnalyzeEditLearningRequest,
    candidate: &LearningReviewCandidate,
) -> bool {
    let original = candidate.original.trim();
    let corrected = candidate.corrected.trim();
    if corrected.is_empty() {
        return false;
    }

    let corrected_in_raw = normalized_text_contains_span(&req.transcript, corrected);
    let original_in_raw =
        !original.is_empty() && normalized_text_contains_span(&req.transcript, original);

    // If the user kept the raw term and AirNote's pasted output changed it,
    // this is polish negative feedback, not an STT alias.
    if corrected_in_raw && !original_in_raw {
        return false;
    }

    // If the edited source is already a protected-looking entity in raw, do
    // not collapse it into another protected entity. Valid company/code swaps
    // are user intent changes or polish feedback, not reusable STT aliases.
    if let Some(raw_source) = raw_surface_for_normalized_span(&req.transcript, original) {
        if looks_like_protected_target(&raw_source)
            && looks_like_protected_target(corrected)
            && normalize_learning_text(&raw_source) != normalize_learning_text(corrected)
        {
            return false;
        }
    }

    true
}

fn raw_surface_for_normalized_span(text: &str, span: &str) -> Option<String> {
    let target_tokens = normalized_tokens(span)
        .into_iter()
        .map(|(_, norm)| norm)
        .collect::<Vec<_>>();
    if target_tokens.is_empty() {
        return None;
    }
    let haystack_tokens = normalized_tokens(text);
    haystack_tokens
        .windows(target_tokens.len())
        .find(|window| window.iter().map(|(_, norm)| norm).eq(target_tokens.iter()))
        .map(|window| {
            window
                .iter()
                .map(|(surface, _)| surface.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        })
}

fn token_overlap_ratio(left: &str, right: &str) -> f32 {
    let left_tokens: std::collections::HashSet<_> = normalize_learning_text(left)
        .split_whitespace()
        .map(str::to_string)
        .collect();
    let right_tokens: std::collections::HashSet<_> = normalize_learning_text(right)
        .split_whitespace()
        .map(str::to_string)
        .collect();
    if left_tokens.is_empty() || right_tokens.is_empty() {
        return 0.0;
    }
    let overlap = left_tokens.intersection(&right_tokens).count();
    overlap as f32 / left_tokens.len().min(right_tokens.len()) as f32
}

fn is_spelled_symbol_span(norm_tokens: &[&str]) -> bool {
    if norm_tokens.len() < 2 {
        return false;
    }
    norm_tokens.iter().all(|token| {
        token.chars().count() == 1
            || matches!(
                *token,
                "zero"
                    | "one"
                    | "two"
                    | "three"
                    | "four"
                    | "five"
                    | "six"
                    | "seven"
                    | "eight"
                    | "nine"
                    | "ten"
            )
    })
}

fn is_learning_action_token(norm: &str) -> bool {
    matches!(
        norm,
        "karo"
            | "karna"
            | "kar"
            | "bhejo"
            | "dikhao"
            | "kholo"
            | "compare"
            | "check"
            | "sync"
            | "export"
            | "clean"
            | "nikalo"
            | "batao"
    )
}

fn looks_like_protected_target(surface: &str) -> bool {
    if looks_like_formatter_only_memory(surface) {
        return false;
    }
    let norm = normalize_learning_text(surface);
    if norm.is_empty() || is_common_learning_term(&norm) {
        return false;
    }
    let trimmed = surface.trim_matches(|c: char| !c.is_alphanumeric() && c != '.');
    if trimmed.chars().any(|c| c.is_ascii_digit()) || trimmed.contains('.') {
        return true;
    }
    let letters: Vec<char> = trimmed.chars().filter(|c| c.is_alphabetic()).collect();
    if letters.len() < 2 {
        return false;
    }
    let upper_count = letters.iter().filter(|c| c.is_uppercase()).count();
    if upper_count == letters.len() && letters.len() <= 8 {
        return true;
    }
    if trimmed
        .chars()
        .next()
        .map(|c| c.is_uppercase())
        .unwrap_or(false)
    {
        return true;
    }
    false
}

fn infer_term_type_from_target(surface: &str) -> &'static str {
    let trimmed = surface.trim();
    if trimmed.chars().any(|c| c.is_ascii_digit()) || trimmed.contains('.') {
        return "code_identifier";
    }
    let letters: Vec<char> = trimmed.chars().filter(|c| c.is_alphabetic()).collect();
    if letters.len() >= 2 && letters.iter().all(|c| c.is_uppercase()) {
        return "acronym";
    }
    "brand"
}

fn is_risky_single_word_alias_source(norm: &str) -> bool {
    matches!(
        norm,
        "cops"
            | "corps"
            | "course"
            | "core"
            | "cause"
            | "cars"
            | "card"
            | "cart"
            | "copy"
            | "copies"
            | "cursor"
            | "agent"
            | "return"
            | "source"
            | "schema"
            | "resolver"
            | "canvas"
            | "bank"
            | "main"
            | "mein"
            | "kaisa"
    )
}

fn levenshtein_chars(left: &str, right: &str) -> usize {
    let a: Vec<char> = left.chars().collect();
    let b: Vec<char> = right.chars().collect();
    let mut costs: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut prev = costs[0];
        costs[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let temp = costs[j + 1];
            let substitution = if ca == cb { prev } else { prev + 1 };
            costs[j + 1] = (costs[j + 1] + 1).min(costs[j] + 1).min(substitution);
            prev = temp;
        }
    }
    costs[b.len()]
}

fn span_similarity(source_norm: &str, target_norm: &str, spelled_symbol: bool) -> f32 {
    let source = source_norm
        .replace("zero", "0")
        .replace("one", "1")
        .replace("two", "2")
        .replace("three", "3")
        .replace("four", "4")
        .replace("five", "5")
        .replace("six", "6")
        .replace("seven", "7")
        .replace("eight", "8")
        .replace("nine", "9")
        .replace("ten", "10");
    let target = target_norm.to_string();
    if source == target || source_norm == target_norm {
        return if spelled_symbol { 0.95 } else { 0.0 };
    }
    let max_len = source.chars().count().max(target.chars().count()).max(1);
    1.0 - (levenshtein_chars(&source, &target) as f32 / max_len as f32)
}

fn deterministic_learning_candidates(
    req: &AnalyzeEditLearningRequest,
) -> Vec<LearningReviewCandidate> {
    if token_overlap_ratio(&req.ai_output, &req.user_kept) < 0.45 {
        return Vec::new();
    }

    let raw_tokens = normalized_tokens(&req.transcript);
    let pasted_tokens = normalized_tokens(&req.ai_output);
    let kept_tokens = normalized_tokens(&req.user_kept);
    let mut out = Vec::new();

    for (kept_index, (target_surface, target_norm)) in kept_tokens.iter().enumerate() {
        if !looks_like_protected_target(target_surface) {
            continue;
        }
        let target_compact = compact_learning_text(target_norm);
        if raw_tokens
            .iter()
            .chain(pasted_tokens.iter())
            .any(|(_, norm)| compact_learning_text(norm) == target_compact)
        {
            continue;
        }
        if pasted_tokens
            .get(kept_index)
            .map(|(_, pasted_norm)| {
                compact_learning_text(pasted_norm) == compact_learning_text(target_norm)
            })
            .unwrap_or(false)
        {
            continue;
        }

        let window_start = kept_index.saturating_sub(2);
        let window_end = (kept_index + 4).min(raw_tokens.len());
        let mut best: Option<(f32, usize, usize)> = None;

        for start in window_start..window_end {
            for len in 1..=4 {
                let end = start + len;
                if end > raw_tokens.len() || end > window_end {
                    continue;
                }
                let span = &raw_tokens[start..end];
                let span_norm = span
                    .iter()
                    .map(|(_, norm)| norm.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                let span_norm_tokens: Vec<&str> = span_norm.split_whitespace().collect();
                if span_norm.is_empty()
                    || is_common_learning_term(&span_norm)
                    || span_norm_tokens
                        .iter()
                        .any(|token| is_learning_action_token(token))
                    || span
                        .iter()
                        .any(|(surface, _)| looks_like_protected_target(surface))
                {
                    continue;
                }
                let span_compact = compact_learning_text(&span_norm);
                let spelled_symbol = is_spelled_symbol_span(&span_norm_tokens);
                let compact_join = span_compact == target_compact;
                if compact_join && len == 1 && !spelled_symbol {
                    continue;
                }
                let similarity = span_similarity(&span_compact, &target_compact, spelled_symbol);
                let enough_signal = similarity >= 0.45 || spelled_symbol || compact_join;
                if !enough_signal {
                    continue;
                }
                let rank = similarity
                    + if compact_join || spelled_symbol {
                        0.20
                    } else {
                        0.0
                    };
                if best
                    .as_ref()
                    .map(|(best_rank, best_start, best_end)| {
                        let best_len = best_end - best_start;
                        rank > *best_rank || ((rank - *best_rank).abs() < 0.01 && len > best_len)
                    })
                    .unwrap_or(true)
                {
                    best = Some((rank, start, end));
                }
            }
        }

        if let Some((_, start, end)) = best {
            let (start, end) =
                expand_learning_source_span(&raw_tokens, start, end, &target_compact);
            let original = raw_tokens[start..end]
                .iter()
                .map(|(surface, _)| surface.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            out.push(LearningReviewCandidate {
                original,
                corrected: target_surface.clone(),
                term_type: infer_term_type_from_target(target_surface).to_string(),
                learnable: true,
                tag: "server_deterministic_alignment".to_string(),
            });
        }
    }

    out
}

fn merge_learning_candidates(
    base: &mut Vec<LearningReviewCandidate>,
    extra: Vec<LearningReviewCandidate>,
) {
    for candidate in extra {
        let corrected_norm = normalize_learning_text(&candidate.corrected);
        if base.iter().any(|existing| {
            existing.learnable && normalize_learning_text(&existing.corrected) == corrected_norm
        }) {
            continue;
        }
        let candidate_key = (normalize_learning_text(&candidate.original), corrected_norm);
        if base.iter().any(|existing| {
            (
                normalize_learning_text(&existing.original),
                normalize_learning_text(&existing.corrected),
            ) == candidate_key
        }) {
            continue;
        }
        base.push(candidate);
    }
}

fn recover_risky_single_word_source_span(
    candidate: &LearningReviewCandidate,
    transcript_tokens: &[(String, String)],
    kept_tokens: &[(String, String)],
) -> Option<String> {
    let source_norm = normalize_learning_text(&candidate.original);
    if count_words(&source_norm) != 1 || !is_risky_single_word_alias_source(&source_norm) {
        return None;
    }
    let corrected_norm = normalize_learning_text(&candidate.corrected);
    let kept_index = kept_tokens
        .iter()
        .position(|(_, norm)| norm == &corrected_norm)?;

    for (idx, (_, norm)) in transcript_tokens.iter().enumerate() {
        if norm != &source_norm {
            continue;
        }
        if idx > 0 {
            let prev = &transcript_tokens[idx - 1];
            let prev_norm = prev.1.as_str();
            let previous_aligns_with_target = idx - 1 == kept_index;
            if previous_aligns_with_target
                && matches!(prev_norm, "main" | "mein" | "mai" | "my" | "me")
            {
                return Some(format!("{} {}", prev.0, transcript_tokens[idx].0));
            }
        }
    }

    None
}

fn expand_learning_source_span(
    raw_tokens: &[(String, String)],
    mut start: usize,
    mut end: usize,
    target_compact: &str,
) -> (usize, usize) {
    let span_compact = |start: usize, end: usize| {
        raw_tokens[start..end]
            .iter()
            .map(|(_, norm)| compact_learning_text(norm))
            .collect::<Vec<_>>()
            .join("")
    };

    if start > 0 {
        let prev_norm = raw_tokens[start - 1].1.as_str();
        let expanded = span_compact(start - 1, end);
        let single_letter_prefix = prev_norm.chars().count() == 1;
        if (!is_common_learning_term(prev_norm) || single_letter_prefix)
            && !is_learning_action_token(prev_norm)
            && (target_compact.ends_with(&span_compact(start, end))
                || target_compact == expanded
                || span_similarity(&expanded, target_compact, false) >= 0.70)
        {
            start -= 1;
        }
    }

    if end < raw_tokens.len() {
        let next_norm = raw_tokens[end].1.as_str();
        let current = span_compact(start, end);
        let expanded = span_compact(start, end + 1);
        if !is_common_learning_term(next_norm)
            && !is_learning_action_token(next_norm)
            && ((current != target_compact
                && target_compact.starts_with(&current)
                && next_norm.chars().count() <= 4)
                || target_compact == expanded
                || span_similarity(&expanded, target_compact, false) >= 0.70)
        {
            end += 1;
        }
    }

    (start, end)
}

fn refine_learning_review_candidates(
    req: &AnalyzeEditLearningRequest,
) -> Vec<LearningReviewCandidate> {
    let transcript_tokens = normalized_tokens(&req.transcript);
    let output_tokens = normalized_tokens(&req.ai_output);
    let kept_tokens = normalized_tokens(&req.user_kept);

    req.candidates
        .iter()
        .cloned()
        .filter_map(|mut candidate| {
            if !candidate.learnable || !is_allowed_term_type(&candidate.term_type) {
                return Some(candidate);
            }
            match trim_unchanged_candidate_context(&candidate) {
                CandidateContextTrim::Unchanged => {}
                CandidateContextTrim::Drop => return None,
                CandidateContextTrim::Trim(trimmed) => candidate = trimmed,
            }
            if looks_like_formatter_only_memory(&candidate.original)
                || looks_like_formatter_only_memory(&candidate.corrected)
            {
                candidate.learnable = false;
                candidate.tag = "server_review_format_memory".to_string();
                return Some(candidate);
            }
            let corrected_norm = normalize_learning_text(&candidate.corrected);
            if corrected_norm.is_empty()
                || is_common_learning_term(&corrected_norm)
                || count_words(&corrected_norm) > 4
            {
                return None;
            }
            if !normalized_text_contains_span(&req.user_kept, &candidate.corrected) {
                return None;
            }
            if !exact_span_candidate_survives_raw_context(req, &candidate) {
                return None;
            }

            let original_norm = normalize_learning_text(&candidate.original);
            if original_norm.is_empty() || is_common_learning_term(&original_norm) {
                if let Some(source) = infer_source_for_corrected(
                    &corrected_norm,
                    &transcript_tokens,
                    &output_tokens,
                    &kept_tokens,
                ) {
                    candidate.original = source;
                }
            }

            if let Some(expanded) =
                recover_risky_single_word_source_span(&candidate, &transcript_tokens, &kept_tokens)
            {
                candidate.original = expanded;
            }
            let final_source_norm = normalize_learning_text(&candidate.original);
            if final_source_norm.is_empty()
                || final_source_norm == corrected_norm
                || is_common_learning_term(&final_source_norm)
                || count_words(&final_source_norm) > 4
            {
                candidate.learnable = false;
                candidate.tag = "server_review_source_missing".to_string();
            }
            Some(candidate)
        })
        .collect()
}

fn normalized_tokens(text: &str) -> Vec<(String, String)> {
    text.split_whitespace()
        .filter_map(|surface| {
            let norm = normalize_learning_text(surface);
            if norm.is_empty() {
                None
            } else {
                Some((
                    surface
                        .trim_matches(|c: char| !c.is_alphanumeric())
                        .to_string(),
                    norm,
                ))
            }
        })
        .collect()
}

fn infer_source_for_corrected(
    corrected_norm: &str,
    transcript_tokens: &[(String, String)],
    output_tokens: &[(String, String)],
    kept_tokens: &[(String, String)],
) -> Option<String> {
    let kept_index = kept_tokens
        .iter()
        .position(|(_, norm)| norm == corrected_norm)?;
    let mut best: Option<(usize, String)> = None;

    for source_tokens in [transcript_tokens, output_tokens] {
        for delta in 0..=3usize {
            for idx in [kept_index.checked_sub(delta), Some(kept_index + delta)] {
                let Some(idx) = idx else { continue };
                let Some((surface, norm)) = source_tokens.get(idx) else {
                    continue;
                };
                if norm == corrected_norm
                    || is_common_learning_term(norm)
                    || count_words(norm) > 4
                    || kept_tokens.iter().any(|(_, kept_norm)| kept_norm == norm)
                {
                    continue;
                }
                let score = delta
                    + if source_tokens.as_ptr() == transcript_tokens.as_ptr() {
                        0
                    } else {
                        1
                    };
                if best
                    .as_ref()
                    .map(|(best_score, _)| score < *best_score)
                    .unwrap_or(true)
                {
                    best = Some((score, surface.clone()));
                }
            }
        }
    }

    best.map(|(_, surface)| surface)
}

fn merge_vocab_terms(request: &[String], server: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut merged = Vec::with_capacity(request.len() + server.len());
    for term in request.iter().chain(server.iter()) {
        let lower = term.to_lowercase();
        if !lower.is_empty() && seen.insert(lower) {
            merged.push(term.clone());
        }
    }
    merged
}

fn apply_exact_resolver(
    output: &str,
    transcript: &str,
    memory: &RuntimeMemory,
) -> (String, usize, usize) {
    if memory.replacements.is_empty() {
        return (output.to_string(), 0, 0);
    }

    let mut result = output.to_string();
    let mut applied = 0usize;
    let mut skipped = 0usize;

    let mut rules: Vec<(&str, &str)> = memory
        .replacements
        .iter()
        .map(|(s, d)| (s.as_str(), d.as_str()))
        .collect();
    rules.sort_by(|(left, _), (right, _)| {
        count_words(right)
            .cmp(&count_words(left))
            .then_with(|| right.len().cmp(&left.len()))
    });

    for (source_form, correct_form) in rules {
        if !is_runtime_exact_alias_safe(source_form, correct_form) {
            skipped += 1;
            continue;
        }
        let correct_norm = normalize_learning_text(correct_form);

        // Skip if output already contains the correct form
        if contains_normalized_phrase(&result, &correct_norm) {
            skipped += 1;
            continue;
        }

        // Only apply if transcript contains the source form as evidence
        if !contains_normalized_phrase(transcript, source_form) {
            skipped += 1;
            continue;
        }

        let new_result = replace_exact_phrase(&result, source_form, correct_form);
        if new_result != result {
            result = new_result;
            applied += 1;
        } else {
            skipped += 1;
        }
    }

    (result, applied, skipped)
}

fn is_runtime_exact_alias_safe(source: &str, correct: &str) -> bool {
    let source = source.trim();
    let correct = correct.trim();
    let source_norm = normalize_learning_text(source);
    let correct_norm = normalize_learning_text(correct);
    if source.len() < 2 || source_norm.is_empty() || correct_norm.is_empty() {
        return false;
    }
    if source_norm == correct_norm {
        return false;
    }
    if count_words(&source_norm) > 4 || count_words(&correct_norm) > 4 {
        return false;
    }
    if is_common_learning_term(&source_norm)
        || is_common_learning_term(&correct_norm)
        || is_risky_single_word_alias_source(&source_norm)
    {
        return false;
    }

    // Mirror stable's conservative "plausible alias" intent: the target should
    // look like a protected/custom term, not an ordinary lowercase word.
    let target_has_protected_shape = correct.chars().any(|c| c.is_ascii_uppercase())
        || correct.chars().any(|c| c.is_ascii_digit())
        || correct
            .chars()
            .any(|c| matches!(c, '_' | '-' | '.' | '@' | '/'));
    target_has_protected_shape || count_words(&source_norm) > 1
}

fn contains_normalized_phrase(text: &str, phrase: &str) -> bool {
    let phrase_tokens = normalized_words(phrase);
    if phrase_tokens.is_empty() {
        return false;
    }
    let text_tokens = normalized_words(text);
    text_tokens
        .windows(phrase_tokens.len())
        .any(|window| window == phrase_tokens.as_slice())
}

fn normalized_words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(trim_token_edges)
        .map(normalize_learning_text)
        .filter(|token| !token.is_empty())
        .collect()
}

fn replace_exact_phrase(text: &str, source: &str, correct: &str) -> String {
    let source_tokens = normalized_words(source);
    if source_tokens.is_empty() {
        return text.to_string();
    }

    let words: Vec<&str> = text.split_whitespace().collect();
    let cores: Vec<String> = words
        .iter()
        .map(|word| normalize_learning_text(trim_token_edges(word)))
        .collect();
    let mut output_words: Vec<String> = Vec::with_capacity(words.len());
    let mut changed = false;

    let mut i = 0usize;
    while i < words.len() {
        let n = source_tokens.len();
        if i + n <= words.len() && cores[i..i + n] == source_tokens[..] {
            let replaced = if n == 1 {
                replace_token_core(words[i], correct)
            } else {
                replace_phrase_core(words[i], words[i + n - 1], correct)
            };
            output_words.push(replaced);
            changed = true;
            i += n;
        } else {
            output_words.push(words[i].to_string());
            i += 1;
        }
    }

    if changed {
        output_words.join(" ")
    } else {
        text.to_string()
    }
}

fn replace_phrase_core(first_word: &str, last_word: &str, correct: &str) -> String {
    let start = first_word
        .char_indices()
        .find(|(_, c)| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))
        .map(|(idx, _)| idx)
        .unwrap_or(0);
    let end = last_word
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))
        .map(|(idx, c)| idx + c.len_utf8())
        .unwrap_or(last_word.len());

    format!("{}{}{}", &first_word[..start], correct, &last_word[end..])
}

async fn load_runtime_memory(
    state: &AppState,
    account_id: Uuid,
) -> Result<RuntimeMemory, sqlx::Error> {
    let vocab_rows: Vec<(String,)> = sqlx::query_as(
        "SELECT term
           FROM personal_vocab_terms
          WHERE account_id = $1 AND status = 'active'
          ORDER BY positive_count DESC, updated_at DESC
          LIMIT 80",
    )
    .bind(account_id)
    .fetch_all(&state.db)
    .await?;

    let replacement_rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT transcript_form, correct_form
           FROM personal_stt_replacements
          WHERE account_id = $1
            AND status = 'active'
            AND safety_status <> 'common_block'
          ORDER BY positive_count DESC, updated_at DESC
          LIMIT 60",
    )
    .bind(account_id)
    .fetch_all(&state.db)
    .await?;

    let policy_rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT variant_form, correct_form
           FROM personal_edit_policy_rules
          WHERE account_id = $1 AND status = 'active'
          ORDER BY positive_count DESC, updated_at DESC
          LIMIT 60",
    )
    .bind(account_id)
    .fetch_all(&state.db)
    .await?;

    Ok(RuntimeMemory {
        vocab_terms: vocab_rows.into_iter().map(|(t,)| t).collect(),
        replacements: replacement_rows,
        policy_rules: policy_rows,
    })
}

async fn judge_and_upsert_client_learning_event(
    state: &AppState,
    user: &AuthUser,
    org_id: Option<Uuid>,
    _run_id: Option<Uuid>,
    req: &ClientEventRequest,
) -> Result<Value, sqlx::Error> {
    if req.event_type.trim() != "classify_edit_result" {
        return Ok(json!({
            "status": "ignored", "accepted_terms": 0, "accepted_aliases": 0,
            "blocked_terms": 0, "blocked_aliases": 0, "ignored": 1,
            "reasons": ["event_type_not_relevant"]
        }));
    }

    let learned = req
        .payload
        .get("learned")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !learned {
        return Ok(json!({
            "status": "ignored", "accepted_terms": 0, "accepted_aliases": 0,
            "blocked_terms": 0, "blocked_aliases": 0, "ignored": 1,
            "reasons": ["learned_false"]
        }));
    }

    let memory = match req.payload.get("memory") {
        Some(m) => m,
        None => {
            return Ok(json!({
                "status": "ignored", "accepted_terms": 0, "accepted_aliases": 0,
                "blocked_terms": 0, "blocked_aliases": 0, "ignored": 1,
                "reasons": ["no_memory_payload"]
            }));
        }
    };

    let accepted_terms_raw = memory
        .get("accepted_terms")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let accepted_aliases_raw = memory
        .get("accepted_aliases")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    if accepted_terms_raw.is_empty() && accepted_aliases_raw.is_empty() {
        return Ok(json!({
            "status": "ignored", "accepted_terms": 0, "accepted_aliases": 0,
            "blocked_terms": 0, "blocked_aliases": 0, "ignored": 1,
            "reasons": ["empty_memory_payload"]
        }));
    }

    let mut accepted_term_count: i64 = 0;
    let mut blocked_term_count: i64 = 0;
    let mut accepted_alias_count: i64 = 0;
    let mut blocked_alias_count: i64 = 0;
    let mut reasons: Vec<String> = Vec::new();

    for term_val in &accepted_terms_raw {
        let term = term_val
            .get("term")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let term_type = term_val
            .get("term_type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let weight = term_val
            .get("weight")
            .and_then(Value::as_f64)
            .unwrap_or(1.0)
            .clamp(0.1, 10.0);
        let source = term_val
            .get("source")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();

        let term_norm = normalize_learning_text(&term);

        if term.is_empty() || source.is_empty() {
            blocked_term_count += 1;
            continue;
        }
        if !is_allowed_term_type(&term_type) {
            blocked_term_count += 1;
            reasons.push(format!("blocked_term_type:{term_type}"));
            continue;
        }
        if looks_like_formatter_only_memory(&term) {
            blocked_term_count += 1;
            reasons.push("blocked_formatter_memory_term".to_string());
            continue;
        }
        if is_common_learning_term(&term_norm) {
            blocked_term_count += 1;
            reasons.push(format!("blocked_common_term:{term_norm}"));
            continue;
        }
        if count_words(&term_norm) > 4 {
            blocked_term_count += 1;
            reasons.push("blocked_term_too_long".to_string());
            continue;
        }

        sqlx::query(
            "INSERT INTO personal_vocab_terms
                 (account_id, org_id, term, term_norm, term_type, source, weight,
                  positive_count, status)
             VALUES ($1, $2, $3, $4, $5, 'desktop_learning', $6, 1, 'active')
             ON CONFLICT (account_id, term_norm) DO UPDATE SET
                 positive_count = personal_vocab_terms.positive_count + 1,
                 weight = GREATEST(personal_vocab_terms.weight, EXCLUDED.weight),
                 status = 'active',
                 last_seen_at = now(),
                 updated_at = now()",
        )
        .bind(user.account_id)
        .bind(org_id)
        .bind(&term)
        .bind(&term_norm)
        .bind(&term_type)
        .bind(weight)
        .execute(&state.db)
        .await?;

        accepted_term_count += 1;
    }

    for alias_val in &accepted_aliases_raw {
        let transcript_form = alias_val
            .get("transcript_form")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let correct_form = alias_val
            .get("correct_form")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let edit_type = alias_val
            .get("edit_type")
            .and_then(Value::as_str)
            .unwrap_or("replace")
            .trim()
            .to_string();
        let source = alias_val
            .get("source")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();

        let transcript_norm = normalize_learning_text(&transcript_form);
        let correct_norm = normalize_learning_text(&correct_form);

        if transcript_form.is_empty() || correct_form.is_empty() || source.is_empty() {
            blocked_alias_count += 1;
            continue;
        }
        if transcript_norm == correct_norm {
            blocked_alias_count += 1;
            reasons.push(format!("blocked_identical_pair:{transcript_norm}"));
            continue;
        }
        if is_common_learning_term(&transcript_norm) {
            blocked_alias_count += 1;
            reasons.push(format!("blocked_common_source:{transcript_norm}"));
            continue;
        }
        if is_common_learning_term(&correct_norm) {
            blocked_alias_count += 1;
            reasons.push(format!("blocked_common_target:{correct_norm}"));
            continue;
        }
        if looks_like_formatter_only_memory(&transcript_form)
            || looks_like_formatter_only_memory(&correct_form)
        {
            blocked_alias_count += 1;
            reasons.push("blocked_formatter_memory_alias".to_string());
            continue;
        }
        if count_words(&transcript_norm) > 4 || count_words(&correct_norm) > 4 {
            blocked_alias_count += 1;
            reasons.push("blocked_alias_too_long".to_string());
            continue;
        }

        sqlx::query(
            "INSERT INTO personal_stt_replacements
                 (account_id, org_id, transcript_form, transcript_norm,
                  correct_form, correct_norm, positive_count, weight,
                  status, safety_status)
             VALUES ($1, $2, $3, $4, $5, $6, 1, 1.0, 'active', 'safe_jargon')
             ON CONFLICT (account_id, transcript_norm, correct_norm) DO UPDATE SET
                 positive_count = personal_stt_replacements.positive_count + 1,
                 weight = GREATEST(personal_stt_replacements.weight, 1.0),
                 status = 'active',
                 last_seen_at = now(),
                 updated_at = now()",
        )
        .bind(user.account_id)
        .bind(org_id)
        .bind(&transcript_form)
        .bind(&transcript_norm)
        .bind(&correct_form)
        .bind(&correct_norm)
        .execute(&state.db)
        .await?;

        // Auto-promote to active when positive_count reaches 2
        sqlx::query(
            "INSERT INTO personal_edit_policy_rules
                 (account_id, org_id, variant_form, variant_norm,
                  correct_form, correct_norm, edit_type, positive_count, status)
             VALUES ($1, $2, $3, $4, $5, $6, $7, 1, 'candidate')
             ON CONFLICT (account_id, variant_norm, correct_norm, edit_type) DO UPDATE SET
                 positive_count = personal_edit_policy_rules.positive_count + 1,
                 status = CASE
                     WHEN personal_edit_policy_rules.positive_count + 1 >= 2 THEN 'active'
                     ELSE personal_edit_policy_rules.status
                 END,
                 last_seen_at = now(),
                 updated_at = now()",
        )
        .bind(user.account_id)
        .bind(org_id)
        .bind(&transcript_form)
        .bind(&transcript_norm)
        .bind(&correct_form)
        .bind(&correct_norm)
        .bind(&edit_type)
        .execute(&state.db)
        .await?;

        accepted_alias_count += 1;
    }

    let total_accepted = accepted_term_count + accepted_alias_count;
    let total_blocked = blocked_term_count + blocked_alias_count;
    if total_accepted > 0 {
        let _ = memory_hygiene::mark_memory_dirty(&state.db, user.account_id).await;
    }
    let status = if total_accepted > 0 && total_blocked == 0 {
        "accepted"
    } else if total_accepted > 0 {
        "partial"
    } else if total_blocked > 0 {
        "blocked"
    } else {
        "ignored"
    };

    Ok(json!({
        "status": status,
        "accepted_terms": accepted_term_count,
        "accepted_aliases": accepted_alias_count,
        "blocked_terms": blocked_term_count,
        "blocked_aliases": blocked_alias_count,
        "ignored": 0,
        "reasons": reasons
    }))
}

fn db_err(e: sqlx::Error) -> (StatusCode, Json<Value>) {
    tracing::warn!("[runtime] database error: {e}");
    json_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
}

fn json_error(status: StatusCode, message: &str) -> (StatusCode, Json<Value>) {
    (
        status,
        Json(json!({ "message": message, "error": message })),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_tone_preset_maps_legacy_and_passes_through() {
        assert_eq!(normalize_tone_preset("work"), "professional");
        assert_eq!(normalize_tone_preset("email"), "professional");
        assert_eq!(normalize_tone_preset("notes"), "concise");
        assert_eq!(normalize_tone_preset("casual"), "casual");
        assert_eq!(normalize_tone_preset("professional"), "professional");
        assert_eq!(normalize_tone_preset("neutral"), "neutral");
    }

    #[test]
    fn voice_polish_request_tone_preset_is_optional_and_additive() {
        // Every existing caller omits tone_preset → defaults to None (behavior unchanged).
        let without: VoicePolishRequest = serde_json::from_str(r#"{"transcript":"hi"}"#).unwrap();
        assert_eq!(without.tone_preset, None);
        // New callers (the keyboard rewrite) can send a per-request tone override.
        let with: VoicePolishRequest =
            serde_json::from_str(r#"{"transcript":"hi","tone_preset":"casual"}"#).unwrap();
        assert_eq!(with.tone_preset.as_deref(), Some("casual"));
    }

    #[test]
    fn restores_product_like_token_replaced_by_model() {
        let output = restore_literal_tokens(
            "Macobs ka pachas percent growth hai",
            "MacBook ka pachas percent growth hai",
            &[],
        );
        assert_eq!(output, "Macobs ka pachas percent growth hai");
    }

    #[test]
    fn preserves_punctuation_when_restoring_literal_token() {
        let output = restore_literal_tokens("Macobs ka update hai", "MacBook, ka update hai", &[]);
        assert_eq!(output, "Macobs, ka update hai");
    }

    #[test]
    fn restores_currency_symbol_flipped_by_model() {
        let output = restore_numeric_literal_tokens(
            "monthly $5 dena padega aur yearly lene par 20% off hai",
            "monthly ₹5 dena padega aur yearly lene par 20% off hai",
        );
        assert_eq!(
            output,
            "monthly $5 dena padega aur yearly lene par 20% off hai"
        );
    }

    #[test]
    fn restores_numeric_literal_punctuation() {
        let output =
            restore_numeric_literal_tokens("Plan $19.99 yearly hai.", "Plan ₹19.99 yearly hai.");
        assert_eq!(output, "Plan $19.99 yearly hai.");
    }

    #[test]
    fn restores_currency_after_post_formatter_if_model_changed_unit_word() {
        let formatted_transcript =
            crate::number_format::apply("monthly five dollar dena padega aur twenty percent off");
        assert_eq!(formatted_transcript, "monthly $5 dena padega aur 20% off");

        let model_output = "monthly 5 rupaye dena padega aur 20% off";
        let post_formatted = crate::number_format::apply(model_output);
        assert_eq!(post_formatted, "monthly ₹5 dena padega aur 20% off");

        let output = restore_numeric_literal_tokens(&formatted_transcript, &post_formatted);
        assert_eq!(output, "monthly $5 dena padega aur 20% off");
    }

    #[test]
    fn server_voice_prompt_forbids_normal_word_translation() {
        let prompt = build_voice_system_prompt("hinglish", "neutral", None, None, &[]);
        let user = build_voice_user_message("hello भाई कैसे हो", "hinglish");

        assert!(prompt.contains("\"hello\" stays \"hello\""));
        assert!(prompt.contains("\"time\" stays \"time\""));
        assert!(prompt.contains("\"kaam\" stays \"kaam\""));
        assert!(prompt.contains("must not become \"Namaste"));
        assert!(user.contains("BEGIN TRANSCRIPT"));
    }

    #[test]
    fn last4_never_returns_more_than_four_chars() {
        assert_eq!(last4("abcdef"), "cdef");
        assert_eq!(last4("abc"), "abc");
    }

    #[test]
    fn runtime_ws_start_metadata_omits_raw_screen_context_and_vocab_values() {
        let raw = json!({
            "screen_context": "super secret existing draft",
            "safe_vocab_terms": ["EMIAC", "Macobs"],
            "audio": {
                "encoding": "linear16",
                "channels": 1
            }
        });
        let metadata = runtime_ws_start_metadata(
            &raw,
            16_000,
            "smart",
            "hinglish",
            2,
            Some("super secret existing draft"),
        );

        assert_eq!(metadata["selected_model"], "smart");
        assert_eq!(metadata["output_language"], "hinglish");
        assert_eq!(metadata["safe_vocab_terms_count"], 2);
        assert_eq!(metadata["screen_context_chars"], 27);
        assert!(metadata.get("screen_context").is_none());
        assert!(metadata.get("safe_vocab_terms").is_none());
    }

    #[test]
    fn runtime_error_message_prefers_message_then_error() {
        let body = Json(json!({
            "message": "credential missing",
            "error": "fallback text"
        }));
        assert_eq!(runtime_error_message(&body), "credential missing");

        let body = Json(json!({
            "error": "deepgram failed"
        }));
        assert_eq!(runtime_error_message(&body), "deepgram failed");
    }

    #[test]
    fn server_review_analyzer_recovers_missing_source_from_transcript() {
        let req = AnalyzeEditLearningRequest {
            recording_id: Some("rec-1".to_string()),
            transcript: "main corps ke ipo ka data laakar de do".to_string(),
            ai_output: "main corps ke ipo ka data laakar de do".to_string(),
            user_kept: "main Macobs ke ipo ka data laakar de do".to_string(),
            candidates: vec![LearningReviewCandidate {
                original: String::new(),
                corrected: "Macobs".to_string(),
                term_type: "brand".to_string(),
                learnable: true,
                tag: "stt".to_string(),
            }],
        };

        let refined = refine_learning_review_candidates(&req);

        assert_eq!(refined.len(), 1);
        assert_eq!(refined[0].original, "corps");
        assert!(refined[0].learnable);
    }

    #[test]
    fn server_review_analyzer_expands_risky_single_word_source() {
        let req = AnalyzeEditLearningRequest {
            recording_id: Some("rec-1".to_string()),
            transcript: "hello bhai main cops ka ipo aa gaya kya".to_string(),
            ai_output: "hello bhai main cops ka ipo aa gaya kya".to_string(),
            user_kept: "hello bhai Macobs ka ipo aa gaya kya".to_string(),
            candidates: vec![LearningReviewCandidate {
                original: "cops".to_string(),
                corrected: "Macobs".to_string(),
                term_type: "brand".to_string(),
                learnable: true,
                tag: "server_llm".to_string(),
            }],
        };

        let refined = refine_learning_review_candidates(&req);

        assert_eq!(refined.len(), 1);
        assert_eq!(refined[0].original, "main cops");
        assert!(refined[0].learnable);
    }

    #[test]
    fn exact_edit_spans_capture_user_typed_target_not_llm_guess() {
        let spans = extract_user_edit_spans(
            "hello bhai Emeic ka update de do",
            "hello bhai Mac ka update de do",
        );

        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].pasted_span, "Emeic");
        assert_eq!(spans[0].kept_span, "Mac");

        let candidates = deterministic_user_edit_span_candidates(&spans);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].original, "Emeic");
        assert_eq!(candidates[0].corrected, "Mac");
        assert_eq!(candidates[0].tag, "server_exact_user_edit_span");
    }

    #[test]
    fn server_review_drops_candidate_if_user_never_typed_target() {
        let req = AnalyzeEditLearningRequest {
            recording_id: Some("rec-1".to_string()),
            transcript: "hello bhai mef ka update de do".to_string(),
            ai_output: "hello bhai Emeic ka update de do".to_string(),
            user_kept: "hello bhai Mac ka update de do".to_string(),
            candidates: vec![LearningReviewCandidate {
                original: "mef".to_string(),
                corrected: "EMIAC".to_string(),
                term_type: "brand".to_string(),
                learnable: true,
                tag: "server_llm".to_string(),
            }],
        };

        assert!(refine_learning_review_candidates(&req).is_empty());
    }

    #[test]
    fn exact_edit_spans_block_polish_revert_and_valid_entity_swap() {
        let polish_revert = AnalyzeEditLearningRequest {
            recording_id: Some("rec-1".to_string()),
            transcript: "Macops deploy ready hai".to_string(),
            ai_output: "Macobs deploy ready hai".to_string(),
            user_kept: "Macops deploy ready hai".to_string(),
            candidates: vec![],
        };
        let spans = extract_user_edit_spans(&polish_revert.ai_output, &polish_revert.user_kept);
        assert_eq!(spans.len(), 1);
        assert!(
            deterministic_user_edit_span_candidates_for_request(&polish_revert, &spans).is_empty()
        );

        let entity_swap = AnalyzeEditLearningRequest {
            recording_id: Some("rec-2".to_string()),
            transcript: "Reliance ka PE compare karo".to_string(),
            ai_output: "Reliance ka PE compare karo".to_string(),
            user_kept: "HDFC Bank ka PE compare karo".to_string(),
            candidates: vec![],
        };
        let spans = extract_user_edit_spans(&entity_swap.ai_output, &entity_swap.user_kept);
        assert_eq!(spans.len(), 1);
        assert!(
            deterministic_user_edit_span_candidates_for_request(&entity_swap, &spans).is_empty()
        );

        let llm_candidate = AnalyzeEditLearningRequest {
            candidates: vec![LearningReviewCandidate {
                original: "Reliance".to_string(),
                corrected: "HDFC Bank".to_string(),
                term_type: "brand".to_string(),
                learnable: true,
                tag: "server_llm".to_string(),
            }],
            ..entity_swap
        };
        assert!(refine_learning_review_candidates(&llm_candidate).is_empty());
    }

    #[test]
    fn exact_edit_spans_capture_multi_word_replacement_together() {
        let spans = extract_user_edit_spans(
            "hello bhai main cops ka IPO aa gaya kya",
            "hello bhai Macobs ka IPO aa gaya kya",
        );

        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].pasted_span, "main cops");
        assert_eq!(spans[0].kept_span, "Macobs");

        let candidates = deterministic_user_edit_span_candidates(&spans);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].original, "main cops");
        assert_eq!(candidates[0].corrected, "Macobs");
    }

    #[test]
    fn exact_edit_spans_handle_three_word_swap_as_one_hunk() {
        let spans = extract_user_edit_spans("please send mef ka ipo", "please send EMIAC ka ipo");

        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].pasted_span, "mef");
        assert_eq!(spans[0].kept_span, "EMIAC");
    }

    #[test]
    fn exact_edit_spans_do_not_make_grammar_edits_learning_candidates() {
        let spans = extract_user_edit_spans("Macobs kaisa laga", "Macobs kaise laga");

        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].pasted_span, "kaisa");
        assert_eq!(spans[0].kept_span, "kaise");
        assert!(deterministic_user_edit_span_candidates(&spans).is_empty());
    }

    #[test]
    fn server_review_analyzer_drops_context_wrapped_common_word_edit() {
        let req = AnalyzeEditLearningRequest {
            recording_id: Some("rec-1".to_string()),
            transcript: "Lark wiki two".to_string(),
            ai_output: "Lark wiki two".to_string(),
            user_kept: "Lark wiki too".to_string(),
            candidates: vec![LearningReviewCandidate {
                original: "Lark wiki two".to_string(),
                corrected: "Lark wiki too".to_string(),
                term_type: "proper_noun".to_string(),
                learnable: true,
                tag: "server_llm".to_string(),
            }],
        };

        assert!(refine_learning_review_candidates(&req).is_empty());
    }

    #[test]
    fn server_review_analyzer_trims_context_wrapped_brand_edit() {
        let req = AnalyzeEditLearningRequest {
            recording_id: Some("rec-1".to_string()),
            transcript: "please kaafka status bhejo".to_string(),
            ai_output: "please kaafka status bhejo".to_string(),
            user_kept: "please Kafka status bhejo".to_string(),
            candidates: vec![LearningReviewCandidate {
                original: "please kaafka".to_string(),
                corrected: "please Kafka".to_string(),
                term_type: "proper_noun".to_string(),
                learnable: true,
                tag: "server_llm".to_string(),
            }],
        };

        let refined = refine_learning_review_candidates(&req);

        assert_eq!(refined.len(), 1);
        assert_eq!(refined[0].original, "kaafka");
        assert_eq!(refined[0].corrected, "Kafka");
        assert!(refined[0].learnable);
        assert_eq!(refined[0].term_type, "brand");
    }

    #[test]
    fn exact_edit_spans_route_email_and_number_formats_away_from_aliases() {
        let email_spans = extract_user_edit_spans(
            "mera email v abhi dot verma at gmail hai",
            "mera email vabhi.verma@gmail.com hai",
        );
        assert!(!email_spans.is_empty());
        assert!(deterministic_user_edit_span_candidates(&email_spans).is_empty());

        let number_spans =
            extract_user_edit_spans("pachas hazaar ka invoice bhejo", "50000 ka invoice bhejo");
        assert_eq!(number_spans.len(), 1);
        assert_eq!(number_spans[0].pasted_span, "pachas hazaar");
        assert_eq!(number_spans[0].kept_span, "50000");
        assert!(deterministic_user_edit_span_candidates(&number_spans).is_empty());
    }

    #[test]
    fn exact_edit_spans_keep_dev_identifier_targets_learnable() {
        let spans = extract_user_edit_spans(
            "n eight n workflow trigger karo",
            "n8n workflow trigger karo",
        );

        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].pasted_span, "n eight n");
        assert_eq!(spans[0].kept_span, "n8n");

        let candidates = deterministic_user_edit_span_candidates(&spans);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].original, "n eight n");
        assert_eq!(candidates[0].corrected, "n8n");
    }

    #[test]
    fn exact_edit_spans_edge_case_matrix_from_research() {
        let cases: Vec<(&str, &str, Vec<(&str, &str)>)> = vec![
            (
                "hello bhai Emeic ka update de do",
                "hello bhai Mac ka update de do",
                vec![("Emeic", "Mac")],
            ),
            (
                "hello bhai main cops ka IPO aa gaya kya",
                "hello bhai Macobs ka IPO aa gaya kya",
                vec![("main cops", "Macobs")],
            ),
            (
                "Mac Robes ka revenue acha tha",
                "Macobs ka revenue acha tha",
                vec![("Mac Robes", "Macobs")],
            ),
            (
                "super base auth laga do",
                "Supabase auth laga do",
                vec![("super base", "Supabase")],
            ),
            (
                "post grass index bana do",
                "Postgres index bana do",
                vec![("post grass", "Postgres")],
            ),
            (
                "graph q l schema bhejo",
                "GraphQL schema bhejo",
                vec![("graph q l", "GraphQL")],
            ),
            (
                "next j s route banao",
                "Next.js route banao",
                vec![("next j s", "Next.js")],
            ),
            (
                "a h refs keyword report bhejo",
                "Ahrefs keyword report bhejo",
                vec![("a h refs", "Ahrefs")],
            ),
            (
                "same rush seo audit bhejo",
                "Semrush seo audit bhejo",
                vec![("same rush", "Semrush")],
            ),
            (
                "mail chimp automation chala do",
                "Mailchimp automation chala do",
                vec![("mail chimp", "Mailchimp")],
            ),
            (
                "perplex city ka answer bhejo",
                "Perplexity ka answer bhejo",
                vec![("perplex city", "Perplexity")],
            ),
            (
                "clever you campaign sync karo",
                "Klaviyo campaign sync karo",
                vec![("clever you", "Klaviyo")],
            ),
            (
                "g a four events check karo",
                "GA4 events check karo",
                vec![("g a four", "GA4")],
            ),
            (
                "r b i circular aa gaya",
                "RBI circular aa gaya",
                vec![("r b i", "RBI")],
            ),
            (
                "n s e ka data lao",
                "NSE ka data lao",
                vec![("n s e", "NSE")],
            ),
            (
                "kal ke IPO mein invest karna hai",
                "kal Macobs ke IPO mein invest karna hai",
                vec![("", "Macobs")],
            ),
            ("Macobs kaisa laga", "Macobs kaise laga", vec![]),
            ("pachas percent discount hai", "50% discount hai", vec![]),
            (
                "five hundred dollars ka invoice bhejo",
                "$500 ka invoice bhejo",
                vec![],
            ),
            (
                "pandrah august tak report bhejo",
                "15 Aug tak report bhejo",
                vec![],
            ),
            (
                "mera email v abhi dot verma at gmail hai",
                "mera email vabhi.verma@gmail.com hai",
                vec![],
            ),
            ("hello world", "Hello, world!", vec![]),
            (
                "client ko bol do kaam ho gaya",
                "Aaron ko update bhej do, Hermes integration complete ho chuka hai",
                vec![],
            ),
        ];

        for (pasted, kept, expected) in cases {
            let spans = extract_user_edit_spans(pasted, kept);
            let candidates = deterministic_user_edit_span_candidates_for_text(pasted, kept, &spans);
            let actual = candidates
                .iter()
                .map(|candidate| (candidate.original.as_str(), candidate.corrected.as_str()))
                .collect::<Vec<_>>();
            assert_eq!(actual, expected, "pasted={pasted:?} kept={kept:?}");
        }
    }

    #[test]
    fn exact_edit_spans_capture_insertions_without_stealing_source() {
        let spans = extract_user_edit_spans("ka data bhejo", "Macobs ka data bhejo");

        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].pasted_span, "");
        assert_eq!(spans[0].kept_span, "Macobs");

        let candidates = deterministic_user_edit_span_candidates(&spans);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].original, "");
        assert_eq!(candidates[0].corrected, "Macobs");
    }

    #[test]
    fn server_review_analyzer_blocks_candidate_when_only_common_source_exists() {
        let req = AnalyzeEditLearningRequest {
            recording_id: Some("rec-1".to_string()),
            transcript: "main ka ipo ka data laakar de do".to_string(),
            ai_output: "main ka ipo ka data laakar de do".to_string(),
            user_kept: "Macobs ka ipo ka data laakar de do".to_string(),
            candidates: vec![LearningReviewCandidate {
                original: String::new(),
                corrected: "Macobs".to_string(),
                term_type: "brand".to_string(),
                learnable: true,
                tag: "stt".to_string(),
            }],
        };

        let refined = refine_learning_review_candidates(&req);

        assert_eq!(refined.len(), 1);
        assert!(!refined[0].learnable);
        assert_eq!(refined[0].tag, "server_review_source_missing");
    }

    #[test]
    fn server_review_analyzer_routes_email_to_formatter_memory() {
        let req = AnalyzeEditLearningRequest {
            recording_id: Some("rec-1".to_string()),
            transcript: "mera email v abhi dot verma at gmail hai".to_string(),
            ai_output: "mera email v abhi dot verma at gmail hai".to_string(),
            user_kept: "mera email vabhi.verma@gmail.com hai".to_string(),
            candidates: vec![LearningReviewCandidate {
                original: "v abhi dot verma at gmail".to_string(),
                corrected: "vabhi.verma@gmail.com".to_string(),
                term_type: "proper_noun".to_string(),
                learnable: true,
                tag: "stt".to_string(),
            }],
        };

        let refined = refine_learning_review_candidates(&req);

        assert_eq!(refined.len(), 1);
        assert!(!refined[0].learnable);
        assert_eq!(refined[0].tag, "server_review_format_memory");
    }

    #[test]
    fn deterministic_learning_candidates_recovers_split_phonetic_terms() {
        let req = AnalyzeEditLearningRequest {
            recording_id: Some("rec-1".to_string()),
            transcript: "post grass index slow hai".to_string(),
            ai_output: "post grass index slow hai".to_string(),
            user_kept: "Postgres index slow hai".to_string(),
            candidates: Vec::new(),
        };

        let candidates = deterministic_learning_candidates(&req);

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].original, "post grass");
        assert_eq!(candidates[0].corrected, "Postgres");
        assert!(candidates[0].learnable);
    }

    #[test]
    fn deterministic_learning_candidates_expands_prefix_and_suffix_spans() {
        let req = AnalyzeEditLearningRequest {
            recording_id: Some("rec-1".to_string()),
            transcript: "graph cute resolver ka schema banao".to_string(),
            ai_output: "graph cute resolver ka schema banao".to_string(),
            user_kept: "GraphQL resolver ka schema banao".to_string(),
            candidates: Vec::new(),
        };

        let candidates = deterministic_learning_candidates(&req);

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].original, "graph cute");
        assert_eq!(candidates[0].corrected, "GraphQL");

        let req = AnalyzeEditLearningRequest {
            recording_id: Some("rec-2".to_string()),
            transcript: "hub spot campaign list sync kar do".to_string(),
            ai_output: "Hub Spot campaign list sync kar do".to_string(),
            user_kept: "HubSpot campaign list sync kar do".to_string(),
            candidates: Vec::new(),
        };

        let candidates = deterministic_learning_candidates(&req);

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].original, "hub spot");
        assert_eq!(candidates[0].corrected, "HubSpot");

        let req = AnalyzeEditLearningRequest {
            recording_id: Some("rec-3".to_string()),
            transcript: "a h refs ka backlink report bhejo".to_string(),
            ai_output: "A H refs ka backlink report bhejo".to_string(),
            user_kept: "Ahrefs ka backlink report bhejo".to_string(),
            candidates: Vec::new(),
        };

        let candidates = deterministic_learning_candidates(&req);

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].original, "a h refs");
        assert_eq!(candidates[0].corrected, "Ahrefs");
    }

    #[test]
    fn deterministic_learning_candidates_recovers_spelled_acronyms() {
        let req = AnalyzeEditLearningRequest {
            recording_id: Some("rec-1".to_string()),
            transcript: "n s e ka option chain kholo".to_string(),
            ai_output: "N S E ka option chain kholo".to_string(),
            user_kept: "NSE ka option chain kholo".to_string(),
            candidates: Vec::new(),
        };

        let candidates = deterministic_learning_candidates(&req);

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].original, "n s e");
        assert_eq!(candidates[0].corrected, "NSE");
        assert!(candidates[0].learnable);
    }

    #[test]
    fn deterministic_learning_candidates_skips_casing_only_common_edits() {
        let req = AnalyzeEditLearningRequest {
            recording_id: Some("rec-1".to_string()),
            transcript: "cursor open karo".to_string(),
            ai_output: "cursor open karo".to_string(),
            user_kept: "Cursor open karo".to_string(),
            candidates: Vec::new(),
        };

        let candidates = deterministic_learning_candidates(&req);

        assert!(candidates.is_empty());
    }

    #[test]
    fn deterministic_learning_candidates_skips_reverted_polish_and_generic_targets() {
        let req = AnalyzeEditLearningRequest {
            recording_id: Some("rec-1".to_string()),
            transcript: "Macops deploy ready hai".to_string(),
            ai_output: "Macobs deploy ready hai".to_string(),
            user_kept: "Macops deploy ready hai".to_string(),
            candidates: Vec::new(),
        };

        assert!(deterministic_learning_candidates(&req).is_empty());

        let req = AnalyzeEditLearningRequest {
            recording_id: Some("rec-2".to_string()),
            transcript: "b s e small cap index nikalo".to_string(),
            ai_output: "B S E small cap index nikalo".to_string(),
            user_kept: "BSE Smallcap index nikalo".to_string(),
            candidates: Vec::new(),
        };

        let candidates = deterministic_learning_candidates(&req);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].original, "b s e");
        assert_eq!(candidates[0].corrected, "BSE");
    }

    #[test]
    fn exact_resolver_uses_only_safe_stt_aliases_not_policy_rules() {
        let memory = RuntimeMemory {
            vocab_terms: vec![],
            replacements: vec![("myak".to_string(), "EMIAC".to_string())],
            policy_rules: vec![("kaisa".to_string(), "Macobs".to_string())],
        };

        let (output, applied, skipped) = apply_exact_resolver(
            "myak ke andar kaam kaisa chal raha hai",
            "myak ke andar kaam kaisa chal raha hai",
            &memory,
        );

        assert_eq!(output, "EMIAC ke andar kaam kaisa chal raha hai");
        assert_eq!(applied, 1);
        assert_eq!(skipped, 0);
    }

    #[test]
    fn exact_resolver_applies_multi_word_alias_longest_first() {
        let memory = RuntimeMemory {
            vocab_terms: vec![],
            replacements: vec![
                ("cops".to_string(), "Wrong".to_string()),
                ("main cops".to_string(), "Macobs".to_string()),
            ],
            policy_rules: vec![],
        };

        let (output, applied, skipped) = apply_exact_resolver(
            "hello bhai main cops, ka IPO aa gaya kya",
            "hello bhai main cops ka IPO aa gaya kya",
            &memory,
        );

        assert_eq!(output, "hello bhai Macobs, ka IPO aa gaya kya");
        assert_eq!(applied, 1);
        assert!(skipped >= 1);
    }

    #[test]
    fn exact_resolver_blocks_risky_single_word_sources_even_if_memory_has_them() {
        let memory = RuntimeMemory {
            vocab_terms: vec![],
            replacements: vec![("cops".to_string(), "Macobs".to_string())],
            policy_rules: vec![],
        };

        let (output, applied, skipped) =
            apply_exact_resolver("cops ka data lao", "cops ka data lao", &memory);

        assert_eq!(output, "cops ka data lao");
        assert_eq!(applied, 0);
        assert_eq!(skipped, 1);
    }
}
