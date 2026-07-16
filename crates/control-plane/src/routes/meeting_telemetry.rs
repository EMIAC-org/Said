//! Metadata-only local meeting telemetry ingestion.
//!
//! Account and organisation attribution always come from the authenticated
//! desktop session plus its active workspace. The request types deliberately do
//! not accept account IDs, organisation IDs, meeting content, or caller-priced
//! dollar amounts.

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{AppState, auth::AuthUser, costs, tenant};

const DEEPSEEK_PROVIDER: &str = "deepseek";
const DEEPSEEK_V4_PRO_MODEL: &str = "deepseek-v4-pro";
const BUNDLED_CREDENTIAL_SCOPE: &str = "airnote_bundled";

type ApiError = (StatusCode, Json<Value>);

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeetingSessionRequest {
    pub client_session_id: String,
    pub status: String,
    pub started_at_ms: i64,
    pub ended_at_ms: i64,
    pub duration_seconds: f64,
    pub transcript_word_count: i32,
    pub transcription_provider: Option<String>,
    pub transcription_model: Option<String>,
    pub transcription_latency_ms: Option<i64>,
    pub device_id: Option<String>,
    pub platform: Option<String>,
    pub app_version: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct MeetingSessionResponse {
    pub ok: bool,
    pub created: bool,
    pub session_id: Uuid,
}

#[derive(Debug)]
struct ValidatedSession {
    client_session_id: String,
    status: String,
    started_at: DateTime<Utc>,
    ended_at: DateTime<Utc>,
    duration_seconds: f64,
    transcript_word_count: i32,
    transcription_provider: Option<String>,
    transcription_model: Option<String>,
    transcription_latency_ms: Option<i64>,
    device_id: Option<String>,
    platform: Option<String>,
    app_version: Option<String>,
}

#[derive(Debug, sqlx::FromRow)]
struct ExistingSession {
    id: Uuid,
    org_id: Uuid,
    status: String,
    started_at: DateTime<Utc>,
    ended_at: DateTime<Utc>,
    duration_seconds: f64,
    transcript_word_count: i32,
    transcription_provider: Option<String>,
    transcription_model: Option<String>,
    transcription_latency_ms: Option<i64>,
    device_id: Option<String>,
    platform: Option<String>,
    app_version: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeetingProviderUsageRequest {
    pub client_session_id: String,
    pub idempotency_key: String,
    pub credential_scope: String,
    pub provider: String,
    pub model: String,
    pub feature_stage: String,
    pub prompt_tokens: i32,
    pub cache_hit_tokens: i32,
    pub cache_miss_tokens: i32,
    pub completion_tokens: i32,
    pub reasoning_tokens: Option<i32>,
    pub latency_ms: i64,
    pub result_status: String,
    pub occurred_at_ms: i64,
}

#[derive(Debug, Serialize)]
pub struct MeetingProviderUsageResponse {
    pub ok: bool,
    pub created: bool,
    pub usage_id: Uuid,
    pub estimated_cost_usd: f64,
    pub cost_source: String,
    pub rate_card_version: String,
    pub rate_card_snapshot: Value,
}

#[derive(Debug)]
struct ValidatedUsage {
    client_session_id: String,
    idempotency_key: String,
    credential_scope: String,
    provider: String,
    model: String,
    feature_stage: String,
    prompt_tokens: i32,
    cache_hit_tokens: i32,
    cache_miss_tokens: i32,
    completion_tokens: i32,
    reasoning_tokens: Option<i32>,
    latency_ms: i64,
    result_status: String,
    occurred_at: DateTime<Utc>,
}

#[derive(Debug, sqlx::FromRow)]
struct ExistingUsage {
    id: Uuid,
    local_meeting_session_id: Uuid,
    org_id: Uuid,
    credential_scope: String,
    provider: String,
    model: String,
    feature_stage: String,
    prompt_tokens: i32,
    cache_hit_tokens: i32,
    cache_miss_tokens: i32,
    completion_tokens: i32,
    reasoning_tokens: Option<i32>,
    latency_ms: i64,
    result_status: String,
    occurred_at: DateTime<Utc>,
    estimated_cost_usd: f64,
    cost_source: String,
    rate_card_version: String,
    rate_card_snapshot: Value,
}

pub async fn ingest_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Json(body): Json<MeetingSessionRequest>,
) -> Result<(StatusCode, Json<MeetingSessionResponse>), ApiError> {
    let (_, org_id) = tenant::require_active_org(&state, &user, &headers).await?;
    let session = validate_session(body).map_err(unprocessable)?;

    let inserted: Option<Uuid> = sqlx::query_scalar(
        "INSERT INTO local_meeting_sessions
            (org_id, account_id, client_session_id, status, started_at, ended_at,
             duration_seconds, transcript_word_count, transcription_provider,
             transcription_model, transcription_latency_ms, device_id, platform, app_version)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)
         ON CONFLICT (account_id, client_session_id) DO NOTHING
         RETURNING id",
    )
    .bind(org_id)
    .bind(user.account_id)
    .bind(&session.client_session_id)
    .bind(&session.status)
    .bind(session.started_at)
    .bind(session.ended_at)
    .bind(session.duration_seconds)
    .bind(session.transcript_word_count)
    .bind(session.transcription_provider.as_deref())
    .bind(session.transcription_model.as_deref())
    .bind(session.transcription_latency_ms)
    .bind(session.device_id.as_deref())
    .bind(session.platform.as_deref())
    .bind(session.app_version.as_deref())
    .fetch_optional(&state.db)
    .await
    .map_err(db_err)?;

    if let Some(session_id) = inserted {
        return Ok((
            StatusCode::CREATED,
            Json(MeetingSessionResponse {
                ok: true,
                created: true,
                session_id,
            }),
        ));
    }

    let existing = fetch_session(&state, user.account_id, &session.client_session_id).await?;
    if existing.org_id != org_id || !same_session(&existing, &session) {
        return Err(conflict(
            "client_session_id was already used with different meeting metadata or workspace",
        ));
    }

    Ok((
        StatusCode::OK,
        Json(MeetingSessionResponse {
            ok: true,
            created: false,
            session_id: existing.id,
        }),
    ))
}

pub async fn ingest_provider_usage(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Json(body): Json<MeetingProviderUsageRequest>,
) -> Result<(StatusCode, Json<MeetingProviderUsageResponse>), ApiError> {
    let (_, org_id) = tenant::require_active_org(&state, &user, &headers).await?;
    let usage = validate_usage(body).map_err(unprocessable)?;

    let existing = fetch_usage(&state, user.account_id, &usage.idempotency_key).await?;
    let session = fetch_session(&state, user.account_id, &usage.client_session_id).await?;
    if session.org_id != org_id {
        return Err(conflict(
            "client_session_id belongs to a different active workspace",
        ));
    }

    if let Some(existing) = existing {
        if existing.org_id != org_id || !same_usage(&existing, session.id, &usage) {
            return Err(conflict(
                "idempotency_key was already used with different provider usage metadata",
            ));
        }
        return Ok((StatusCode::OK, Json(usage_response(existing, false))));
    }

    let estimated_cost_usd = costs::deepseek_v4_pro_cost(
        usage.cache_hit_tokens,
        usage.cache_miss_tokens,
        usage.completion_tokens,
    )
    .ok_or_else(|| unprocessable("token counts must be non-negative"))?;
    let rate_card_snapshot = deepseek_v4_pro_rate_card_snapshot();

    let inserted: Option<Uuid> = sqlx::query_scalar(
        "INSERT INTO local_meeting_provider_usage
            (local_meeting_session_id, org_id, account_id, idempotency_key,
             credential_scope, provider, model, feature_stage, prompt_tokens,
             cache_hit_tokens, cache_miss_tokens, completion_tokens, reasoning_tokens,
             latency_ms, result_status, occurred_at, estimated_cost_usd, cost_source,
             rate_card_version, rate_card_snapshot)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20)
         ON CONFLICT (account_id, idempotency_key) DO NOTHING
         RETURNING id",
    )
    .bind(session.id)
    .bind(org_id)
    .bind(user.account_id)
    .bind(&usage.idempotency_key)
    .bind(&usage.credential_scope)
    .bind(&usage.provider)
    .bind(&usage.model)
    .bind(&usage.feature_stage)
    .bind(usage.prompt_tokens)
    .bind(usage.cache_hit_tokens)
    .bind(usage.cache_miss_tokens)
    .bind(usage.completion_tokens)
    .bind(usage.reasoning_tokens)
    .bind(usage.latency_ms)
    .bind(&usage.result_status)
    .bind(usage.occurred_at)
    .bind(estimated_cost_usd)
    .bind(costs::DEEPSEEK_V4_PRO_RATE_SOURCE)
    .bind(costs::DEEPSEEK_V4_PRO_RATE_VERSION)
    .bind(&rate_card_snapshot)
    .fetch_optional(&state.db)
    .await
    .map_err(db_err)?;

    if let Some(usage_id) = inserted {
        return Ok((
            StatusCode::CREATED,
            Json(MeetingProviderUsageResponse {
                ok: true,
                created: true,
                usage_id,
                estimated_cost_usd,
                cost_source: costs::DEEPSEEK_V4_PRO_RATE_SOURCE.to_string(),
                rate_card_version: costs::DEEPSEEK_V4_PRO_RATE_VERSION.to_string(),
                rate_card_snapshot,
            }),
        ));
    }

    // A concurrent retry may have won the unique-key race after our first
    // lookup. Read the committed row and apply the same mutation guard.
    let existing = fetch_usage(&state, user.account_id, &usage.idempotency_key)
        .await?
        .ok_or_else(|| db_message("provider usage conflict row disappeared"))?;
    if existing.org_id != org_id || !same_usage(&existing, session.id, &usage) {
        return Err(conflict(
            "idempotency_key was already used with different provider usage metadata",
        ));
    }
    Ok((StatusCode::OK, Json(usage_response(existing, false))))
}

async fn fetch_session(
    state: &AppState,
    account_id: Uuid,
    client_session_id: &str,
) -> Result<ExistingSession, ApiError> {
    sqlx::query_as(
        "SELECT id, org_id, status, started_at, ended_at, duration_seconds,
                transcript_word_count, transcription_provider, transcription_model,
                transcription_latency_ms, device_id, platform, app_version
           FROM local_meeting_sessions
          WHERE account_id = $1 AND client_session_id = $2",
    )
    .bind(account_id)
    .bind(client_session_id)
    .fetch_optional(&state.db)
    .await
    .map_err(db_err)?
    .ok_or_else(|| not_found("local meeting session not found"))
}

async fn fetch_usage(
    state: &AppState,
    account_id: Uuid,
    idempotency_key: &str,
) -> Result<Option<ExistingUsage>, ApiError> {
    sqlx::query_as(
        "SELECT id, local_meeting_session_id, org_id, credential_scope, provider,
                model, feature_stage, prompt_tokens, cache_hit_tokens,
                cache_miss_tokens, completion_tokens, reasoning_tokens, latency_ms,
                result_status, occurred_at, estimated_cost_usd, cost_source,
                rate_card_version, rate_card_snapshot
           FROM local_meeting_provider_usage
          WHERE account_id = $1 AND idempotency_key = $2",
    )
    .bind(account_id)
    .bind(idempotency_key)
    .fetch_optional(&state.db)
    .await
    .map_err(db_err)
}

fn validate_session(body: MeetingSessionRequest) -> Result<ValidatedSession, &'static str> {
    let client_session_id = clean_key(&body.client_session_id, 128)
        .ok_or("client_session_id must be 1-128 safe identifier characters")?;
    let status = body.status.trim().to_ascii_lowercase();
    if !matches!(status.as_str(), "completed" | "failed" | "cancelled") {
        return Err("status must be completed, failed, or cancelled");
    }
    let started_at = DateTime::from_timestamp_millis(body.started_at_ms)
        .ok_or("started_at_ms is outside the supported timestamp range")?;
    let ended_at = DateTime::from_timestamp_millis(body.ended_at_ms)
        .ok_or("ended_at_ms is outside the supported timestamp range")?;
    if ended_at < started_at {
        return Err("ended_at_ms must not be before started_at_ms");
    }
    if !body.duration_seconds.is_finite() || body.duration_seconds < 0.0 {
        return Err("duration_seconds must be finite and non-negative");
    }
    if body.transcript_word_count < 0 {
        return Err("transcript_word_count must be non-negative");
    }
    if body.transcription_latency_ms.is_some_and(|value| value < 0) {
        return Err("transcription_latency_ms must be non-negative");
    }
    Ok(ValidatedSession {
        client_session_id,
        status,
        started_at,
        ended_at,
        duration_seconds: body.duration_seconds,
        transcript_word_count: body.transcript_word_count,
        transcription_provider: clean_optional(body.transcription_provider, 64)?,
        transcription_model: clean_optional(body.transcription_model, 128)?,
        transcription_latency_ms: body.transcription_latency_ms,
        device_id: clean_optional(body.device_id, 256)?,
        platform: clean_optional(body.platform, 64)?,
        app_version: clean_optional(body.app_version, 64)?,
    })
}

fn validate_usage(body: MeetingProviderUsageRequest) -> Result<ValidatedUsage, &'static str> {
    let client_session_id = clean_key(&body.client_session_id, 128)
        .ok_or("client_session_id must be 1-128 safe identifier characters")?;
    let idempotency_key = clean_key(&body.idempotency_key, 200)
        .ok_or("idempotency_key must be 1-200 safe identifier characters")?;
    let credential_scope = body.credential_scope.trim().to_ascii_lowercase();
    if credential_scope != BUNDLED_CREDENTIAL_SCOPE {
        return Err("only airnote_bundled DeepSeek usage is accepted");
    }
    let provider = body.provider.trim().to_ascii_lowercase();
    if provider != DEEPSEEK_PROVIDER {
        return Err("provider must be deepseek");
    }
    let model = body.model.trim().to_ascii_lowercase();
    if model != DEEPSEEK_V4_PRO_MODEL {
        return Err("model must be deepseek-v4-pro");
    }
    let feature_stage = clean_key(&body.feature_stage, 64)
        .ok_or("feature_stage must be 1-64 safe identifier characters")?;
    if body.prompt_tokens < 0
        || body.cache_hit_tokens < 0
        || body.cache_miss_tokens < 0
        || body.completion_tokens < 0
    {
        return Err("token counts must be non-negative");
    }
    let cache_total = body
        .cache_hit_tokens
        .checked_add(body.cache_miss_tokens)
        .ok_or("cache token counts overflow")?;
    if body.prompt_tokens != cache_total {
        return Err("prompt_tokens must equal cache_hit_tokens + cache_miss_tokens");
    }
    if body
        .reasoning_tokens
        .is_some_and(|value| value < 0 || value > body.completion_tokens)
    {
        return Err("reasoning_tokens must be non-negative and no greater than completion_tokens");
    }
    if body.latency_ms < 0 {
        return Err("latency_ms must be non-negative");
    }
    let result_status = body.result_status.trim().to_ascii_lowercase();
    if !matches!(
        result_status.as_str(),
        "success" | "error" | "cancelled" | "timeout"
    ) {
        return Err("result_status must be success, error, cancelled, or timeout");
    }
    let occurred_at = DateTime::from_timestamp_millis(body.occurred_at_ms)
        .ok_or("occurred_at_ms is outside the supported timestamp range")?;

    Ok(ValidatedUsage {
        client_session_id,
        idempotency_key,
        credential_scope,
        provider,
        model,
        feature_stage,
        prompt_tokens: body.prompt_tokens,
        cache_hit_tokens: body.cache_hit_tokens,
        cache_miss_tokens: body.cache_miss_tokens,
        completion_tokens: body.completion_tokens,
        reasoning_tokens: body.reasoning_tokens,
        latency_ms: body.latency_ms,
        result_status,
        occurred_at,
    })
}

fn clean_key(value: &str, max_len: usize) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()
        && value.len() <= max_len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':')))
    .then(|| value.to_string())
}

fn clean_optional(value: Option<String>, max_len: usize) -> Result<Option<String>, &'static str> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value.len() > max_len {
        return Err("optional metadata field exceeds its maximum length");
    }
    Ok(Some(value.to_string()))
}

fn same_session(existing: &ExistingSession, incoming: &ValidatedSession) -> bool {
    existing.status == incoming.status
        && existing.started_at == incoming.started_at
        && existing.ended_at == incoming.ended_at
        && existing.duration_seconds.to_bits() == incoming.duration_seconds.to_bits()
        && existing.transcript_word_count == incoming.transcript_word_count
        && existing.transcription_provider == incoming.transcription_provider
        && existing.transcription_model == incoming.transcription_model
        && existing.transcription_latency_ms == incoming.transcription_latency_ms
        && existing.device_id == incoming.device_id
        && existing.platform == incoming.platform
        && existing.app_version == incoming.app_version
}

fn same_usage(existing: &ExistingUsage, session_id: Uuid, incoming: &ValidatedUsage) -> bool {
    existing.local_meeting_session_id == session_id
        && existing.credential_scope == incoming.credential_scope
        && existing.provider == incoming.provider
        && existing.model == incoming.model
        && existing.feature_stage == incoming.feature_stage
        && existing.prompt_tokens == incoming.prompt_tokens
        && existing.cache_hit_tokens == incoming.cache_hit_tokens
        && existing.cache_miss_tokens == incoming.cache_miss_tokens
        && existing.completion_tokens == incoming.completion_tokens
        && existing.reasoning_tokens == incoming.reasoning_tokens
        && existing.latency_ms == incoming.latency_ms
        && existing.result_status == incoming.result_status
        && existing.occurred_at == incoming.occurred_at
}

fn deepseek_v4_pro_rate_card_snapshot() -> Value {
    json!({
        "version": costs::DEEPSEEK_V4_PRO_RATE_VERSION,
        "currency": "USD",
        "unit_tokens": 1_000_000,
        "provider": DEEPSEEK_PROVIDER,
        "model": DEEPSEEK_V4_PRO_MODEL,
        "captured_on": "2026-07-17",
        "source_url": costs::DEEPSEEK_V4_PRO_PRICING_URL,
        "cache_hit_usd_per_million": costs::DEEPSEEK_V4_PRO_CACHE_HIT_USD_PER_MILLION,
        "cache_miss_usd_per_million": costs::DEEPSEEK_V4_PRO_CACHE_MISS_USD_PER_MILLION,
        "output_usd_per_million": costs::DEEPSEEK_V4_PRO_OUTPUT_USD_PER_MILLION,
    })
}

fn usage_response(existing: ExistingUsage, created: bool) -> MeetingProviderUsageResponse {
    MeetingProviderUsageResponse {
        ok: true,
        created,
        usage_id: existing.id,
        estimated_cost_usd: existing.estimated_cost_usd,
        cost_source: existing.cost_source,
        rate_card_version: existing.rate_card_version,
        rate_card_snapshot: existing.rate_card_snapshot,
    }
}

fn unprocessable(message: &'static str) -> ApiError {
    json_error(StatusCode::UNPROCESSABLE_ENTITY, message)
}

fn conflict(message: &'static str) -> ApiError {
    json_error(StatusCode::CONFLICT, message)
}

fn not_found(message: &'static str) -> ApiError {
    json_error(StatusCode::NOT_FOUND, message)
}

fn db_message(message: &'static str) -> ApiError {
    tracing::warn!("[meeting-telemetry] {message}");
    json_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
}

fn db_err(error: sqlx::Error) -> ApiError {
    tracing::warn!("[meeting-telemetry] database error: {error}");
    json_error(StatusCode::INTERNAL_SERVER_ERROR, "database error")
}

fn json_error(status: StatusCode, message: &'static str) -> ApiError {
    (status, Json(json!({ "error": message })))
}

#[cfg(test)]
mod tests {
    use super::{
        MeetingProviderUsageRequest, MeetingSessionRequest, validate_session, validate_usage,
    };

    fn valid_session() -> MeetingSessionRequest {
        MeetingSessionRequest {
            client_session_id: "meeting-123".into(),
            status: "completed".into(),
            started_at_ms: 1_700_000_000_000,
            ended_at_ms: 1_700_000_060_000,
            duration_seconds: 60.0,
            transcript_word_count: 140,
            transcription_provider: Some("local_whisper".into()),
            transcription_model: Some("ggml-large-v3-turbo".into()),
            transcription_latency_ms: Some(2_500),
            device_id: Some("device-1".into()),
            platform: Some("macos".into()),
            app_version: Some("3.0.0".into()),
        }
    }

    fn valid_usage() -> MeetingProviderUsageRequest {
        MeetingProviderUsageRequest {
            client_session_id: "meeting-123".into(),
            idempotency_key: "provider-call-456".into(),
            credential_scope: "airnote_bundled".into(),
            provider: "deepseek".into(),
            model: "deepseek-v4-pro".into(),
            feature_stage: "summary".into(),
            prompt_tokens: 1_000,
            cache_hit_tokens: 400,
            cache_miss_tokens: 600,
            completion_tokens: 200,
            reasoning_tokens: None,
            latency_ms: 1_500,
            result_status: "success".into(),
            occurred_at_ms: 1_700_000_060_000,
        }
    }

    #[test]
    fn validates_metadata_only_completed_session() {
        let session = validate_session(valid_session()).expect("valid session");
        assert_eq!(session.client_session_id, "meeting-123");
        assert_eq!(session.transcript_word_count, 140);
    }

    #[test]
    fn rejects_invalid_session_timing_and_counts() {
        let mut request = valid_session();
        request.ended_at_ms = request.started_at_ms - 1;
        assert!(validate_session(request).is_err());

        let mut request = valid_session();
        request.transcript_word_count = -1;
        assert!(validate_session(request).is_err());
    }

    #[test]
    fn validates_cache_split_and_bundled_v4_pro_scope() {
        let usage = validate_usage(valid_usage()).expect("valid usage");
        assert_eq!(usage.prompt_tokens, 1_000);
        assert_eq!(usage.model, "deepseek-v4-pro");
    }

    #[test]
    fn rejects_non_bundled_or_inconsistent_provider_usage() {
        let mut request = valid_usage();
        request.credential_scope = "developer_override".into();
        assert!(validate_usage(request).is_err());

        let mut request = valid_usage();
        request.cache_miss_tokens = 599;
        assert!(validate_usage(request).is_err());

        let mut request = valid_usage();
        request.model = "deepseek-v4-flash".into();
        assert!(validate_usage(request).is_err());
    }
}
