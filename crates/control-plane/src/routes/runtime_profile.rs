//! Server-owned runtime user profiles — read/patch/rebuild API skeletons.

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    AppState,
    auth::AuthUser,
    profile::store::{self, ProfilePatch, ProfileRow, validate_profile_sizes},
    profile::updater::{
        deepseek,
        jobs::{build_profile_update_request, enqueue_learn_job},
        run_resolve::resolve_run_id_for_learn,
        types::{
            DeepSeekClassification, DeepSeekMarkdownPatch, DeepSeekProfilePatch,
            DeepSeekProfileUpdateResponse, LearnAuditPayload, LearnFromEditRequest,
            LearnFromEditResponse, ProfileUpdateEdit,
        },
        validator::{ValidatorDecision, ValidatorInput, validate_and_merge},
    },
    profile::{
        self, CachedRuntimeProfile, ProfileCacheKey, invalidate_profile_cache, resolve_org_scope,
    },
    routes::runtime::compute_user_edit_spans,
    tenant,
};

#[derive(Debug, Serialize)]
pub struct ProfileResponse {
    pub account_id: Uuid,
    pub org_scope: Uuid,
    pub profile_json: Value,
    pub profile_markdown: String,
    pub version: i64,
    pub schema_version: i32,
    pub status: String,
    pub source_hash: String,
    pub dirty_at: Option<DateTime<Utc>>,
    pub last_rebuilt_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct PatchProfileRequest {
    pub profile_json: Option<Value>,
    pub profile_markdown: Option<String>,
    pub mark_dirty: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct RebuildProfileResponse {
    pub status: String,
    pub message: String,
    pub profile: ProfileResponse,
}

#[derive(Debug, Serialize)]
pub struct ProfileMemoryResponse {
    pub profile: ProfileResponse,
    pub pending_proposals: Vec<ProfileLearningProposal>,
    pub stable_terms: Vec<ProfileMemoryTerm>,
    pub aliases: Vec<ProfileMemoryAlias>,
    pub domains: Vec<ProfileMemoryDomain>,
}

#[derive(Debug, Serialize)]
pub struct ProfileLearningProposal {
    pub job_id: Uuid,
    pub edit_event_id: String,
    pub status: String,
    pub from_version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub ai_output: String,
    pub user_kept: String,
    pub raw_transcript: Option<String>,
    pub classification: Option<String>,
    pub confidence: Option<f64>,
    pub reason: Option<String>,
    pub delta_summary: Value,
    pub stable_terms: Vec<ProfileMemoryTerm>,
    pub aliases: Vec<ProfileMemoryAlias>,
    pub domains: Vec<ProfileMemoryDomain>,
}

#[derive(Debug, Serialize)]
pub struct ProfileMemoryTerm {
    pub term: String,
    pub term_type: Option<String>,
    pub evidence: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ProfileMemoryAlias {
    pub source_phrase: String,
    pub canonical_phrase: String,
    pub status: String,
    pub confidence: Option<f64>,
    pub evidence_count: Option<i32>,
    pub reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ProfileMemoryDomain {
    pub name: String,
    pub weight: Option<f64>,
    pub evidence: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ProfileProposalDecisionResponse {
    pub job_id: Uuid,
    pub status: String,
    pub profile_version: Option<i64>,
    pub message: String,
}

#[derive(Debug, sqlx::FromRow)]
struct ProfileJobReviewRow {
    pub id: Uuid,
    pub edit_event_id: String,
    pub status: String,
    pub request_json: Value,
    pub response_json: Option<Value>,
    pub from_version: i64,
    pub to_version: Option<i64>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn empty_profile_response(account_id: Uuid, org_scope: Uuid) -> ProfileResponse {
    ProfileResponse {
        account_id,
        org_scope,
        profile_json: json!({}),
        profile_markdown: String::new(),
        version: 0,
        schema_version: 1,
        status: "ready".to_string(),
        source_hash: String::new(),
        dirty_at: None,
        last_rebuilt_at: None,
        last_error: None,
        updated_at: Utc::now(),
    }
}

fn into_response(row: ProfileRow) -> ProfileResponse {
    ProfileResponse {
        account_id: row.account_id,
        org_scope: row.org_scope,
        profile_json: row.profile_json,
        profile_markdown: row.profile_markdown,
        version: row.version,
        schema_version: row.schema_version,
        status: row.status,
        source_hash: row.source_hash,
        dirty_at: row.dirty_at,
        last_rebuilt_at: row.last_rebuilt_at,
        last_error: row.last_error,
        updated_at: row.updated_at,
    }
}

async fn resolve_scope(
    state: &AppState,
    user: &AuthUser,
    headers: &HeaderMap,
) -> Result<Uuid, (StatusCode, Json<Value>)> {
    let tenant = tenant::resolve_tenant(state, user, headers).await?;
    Ok(resolve_org_scope(&tenant))
}

fn internal_error<E: std::fmt::Display>(e: E) -> (StatusCode, Json<Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": e.to_string()})),
    )
}

fn conflict(message: impl Into<String>) -> (StatusCode, Json<Value>) {
    (StatusCode::CONFLICT, Json(json!({"error": message.into()})))
}

pub async fn get_profile(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
) -> Result<Json<ProfileResponse>, (StatusCode, Json<Value>)> {
    let org_scope = resolve_scope(&state, &user, &headers).await?;
    let cache_key = ProfileCacheKey {
        account_id: user.account_id,
        org_scope,
    };
    let cache_hit = state.profile_cache.get(&cache_key).is_some();

    let row = store::get_profile_with_fallback(&state.db, user.account_id, org_scope)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?;

    let response = match row {
        Some(row) => into_response(row),
        None => empty_profile_response(user.account_id, org_scope),
    };

    tracing::info!(
        "[profile] get account={} org_scope={} version={} status={} cache_hit={}",
        user.account_id,
        org_scope,
        response.version,
        response.status,
        cache_hit,
    );

    Ok(Json(response))
}

pub async fn get_profile_memory(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
) -> Result<Json<ProfileMemoryResponse>, (StatusCode, Json<Value>)> {
    let org_scope = resolve_scope(&state, &user, &headers).await?;
    store::ensure_profile_row(&state.db, user.account_id, org_scope)
        .await
        .map_err(internal_error)?;

    let row = store::get_profile(&state.db, user.account_id, org_scope)
        .await
        .map_err(internal_error)?
        .unwrap_or_else(|| ProfileRow {
            account_id: user.account_id,
            org_scope,
            profile_json: json!({}),
            profile_markdown: String::new(),
            version: 0,
            schema_version: 1,
            status: "ready".to_string(),
            source_hash: String::new(),
            dirty_at: None,
            last_rebuilt_at: None,
            last_error: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        });
    let profile_response = into_response(row.clone());
    let proposals = list_pending_proposals(&state, user.account_id, org_scope).await?;
    tracing::info!(
        "[profile] memory get account={} org_scope={} version={} status={} pending={} terms={} aliases={} domains={}",
        user.account_id,
        org_scope,
        profile_response.version,
        profile_response.status,
        proposals.len(),
        stable_terms_from_profile(&row.profile_json).len(),
        aliases_from_profile(&row.profile_json).len(),
        domains_from_profile(&row.profile_json).len(),
    );

    Ok(Json(ProfileMemoryResponse {
        stable_terms: stable_terms_from_profile(&row.profile_json),
        aliases: aliases_from_profile(&row.profile_json),
        domains: domains_from_profile(&row.profile_json),
        profile: profile_response,
        pending_proposals: proposals,
    }))
}

pub async fn patch_profile(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Json(req): Json<PatchProfileRequest>,
) -> Result<Json<ProfileResponse>, (StatusCode, Json<Value>)> {
    let org_scope = resolve_scope(&state, &user, &headers).await?;

    if req.profile_json.is_none()
        && req.profile_markdown.is_none()
        && !req.mark_dirty.unwrap_or(false)
    {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "no profile fields to update"})),
        ));
    }

    store::ensure_profile_row(&state.db, user.account_id, org_scope)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?;

    let current = store::get_profile(&state.db, user.account_id, org_scope)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?
        .unwrap_or_else(|| ProfileRow {
            account_id: user.account_id,
            org_scope,
            profile_json: json!({}),
            profile_markdown: String::new(),
            version: 0,
            schema_version: 1,
            status: "ready".to_string(),
            source_hash: String::new(),
            dirty_at: None,
            last_rebuilt_at: None,
            last_error: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        });

    let next_json = req.profile_json.unwrap_or(current.profile_json);
    let next_markdown = req
        .profile_markdown
        .unwrap_or(current.profile_markdown.clone());
    validate_profile_sizes(&next_json, &next_markdown)
        .map_err(|e| (StatusCode::BAD_REQUEST, Json(json!({"error": e}))))?;

    let row = store::upsert_profile_patch(
        &state.db,
        user.account_id,
        org_scope,
        ProfilePatch {
            profile_json: Some(next_json),
            profile_markdown: Some(next_markdown),
            mark_dirty: req.mark_dirty.unwrap_or(false),
            source: "api",
        },
    )
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
    })?;

    invalidate_profile_cache(
        &state,
        &ProfileCacheKey {
            account_id: user.account_id,
            org_scope,
        },
    );

    tracing::info!(
        "[profile] patch account={} org_scope={} version={} status={}",
        user.account_id,
        org_scope,
        row.version,
        row.status,
    );

    Ok(Json(into_response(row)))
}

pub async fn rebuild_profile(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
) -> Result<Json<RebuildProfileResponse>, (StatusCode, Json<Value>)> {
    let org_scope = resolve_scope(&state, &user, &headers).await?;

    store::ensure_profile_row(&state.db, user.account_id, org_scope)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?;

    let row = store::mark_profile_rebuilding(&state.db, user.account_id, org_scope)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?;

    invalidate_profile_cache(
        &state,
        &ProfileCacheKey {
            account_id: user.account_id,
            org_scope,
        },
    );

    tracing::info!(
        "[profile] rebuild queued account={} org_scope={} version={} status={}",
        user.account_id,
        org_scope,
        row.version,
        row.status,
    );

    Ok(Json(RebuildProfileResponse {
        status: "queued".to_string(),
        message: "rebuild not enabled".to_string(),
        profile: into_response(row),
    }))
}

pub async fn learn_from_edit(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Json(req): Json<LearnFromEditRequest>,
) -> Result<(StatusCode, Json<LearnFromEditResponse>), (StatusCode, Json<Value>)> {
    // Canonical profile learning path — writes only to runtime_user_profiles via
    // the async profile-updater worker. Does not call client_event, confirm-batch,
    // judge_and_upsert_client_learning_event, or any personal_* table writers.
    let edit_event_id = req.edit_event_id.trim();
    if edit_event_id.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "edit_event_id is required"})),
        ));
    }
    if req.ai_output.trim().is_empty() || req.user_kept.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "ai_output and user_kept are required"})),
        ));
    }

    let org_scope = resolve_scope(&state, &user, &headers).await?;

    store::ensure_profile_row(&state.db, user.account_id, org_scope)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?;

    let current = store::get_profile(&state.db, user.account_id, org_scope)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        })?;

    let from_version = current.as_ref().map(|r| r.version).unwrap_or(0);

    // Idempotent: return existing job without duplicating audit or queue entry.
    if let Some(existing_id) = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM runtime_profile_learn_jobs
          WHERE account_id = $1 AND org_scope = $2 AND edit_event_id = $3",
    )
    .bind(user.account_id)
    .bind(org_scope)
    .bind(edit_event_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
    })? {
        tracing::info!(
            "[profile] learn-from-edit idempotent-hit account={} org_scope={} edit_event={} existing_job={} from_version={}",
            user.account_id,
            org_scope,
            edit_event_id,
            existing_id,
            from_version,
        );
        return Ok((
            StatusCode::ACCEPTED,
            Json(LearnFromEditResponse {
                job_id: existing_id,
                status: "queued",
                message: Some("existing job returned for edit_event_id".into()),
            }),
        ));
    }

    let job_id = uuid::Uuid::new_v4();

    let (client_run_id, resolved_run_id) = resolve_run_id_for_learn(
        &state.db,
        user.account_id,
        org_scope,
        req.client_run_id.as_deref(),
        req.run_id,
    )
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
    })?;

    let queued_audit = LearnAuditPayload {
        edit_event_id: edit_event_id.to_string(),
        recording_id: req.recording_id.clone(),
        client_run_id: client_run_id.clone(),
        run_id: resolved_run_id,
        job_id: Some(job_id),
        deepseek_classification: None,
        deepseek_confidence: None,
        deepseek_reason: None,
        validator_decision: None,
        validator_reasons: None,
        alias_changes: None,
        profile_json_delta_summary: None,
        deepseek_request_id: None,
        latency_ms: None,
        shadow_would_apply: None,
    };
    let audit_value = serde_json::to_value(&queued_audit).unwrap_or_else(|_| json!({}));
    store::write_profile_audit(
        &state.db,
        user.account_id,
        org_scope,
        from_version,
        from_version,
        "learn_queued",
        audit_value,
        "api",
    )
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
    })?;

    let edit_spans = compute_user_edit_spans(&req.ai_output, &req.user_kept);
    tracing::info!(
        "[profile] learn-from-edit received account={} org_scope={} edit_event={} from_version={} recording_id={} client_run_id={} resolved_run_id={} ai_chars={} kept_chars={} raw_chars={} edit_spans={} target_app={} model_used={} ai_preview=\"{}\" kept_preview=\"{}\"",
        user.account_id,
        org_scope,
        edit_event_id,
        from_version,
        req.recording_id.as_deref().unwrap_or("none"),
        client_run_id.as_deref().unwrap_or("none"),
        resolved_run_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| "none".to_string()),
        req.ai_output.chars().count(),
        req.user_kept.chars().count(),
        req.raw_transcript
            .as_deref()
            .map(|s| s.chars().count())
            .unwrap_or(0),
        edit_spans.len(),
        req.target_app.as_deref().unwrap_or("none"),
        req.model_used.as_deref().unwrap_or("none"),
        said_core::text::truncate_utf8(&req.ai_output, 120),
        said_core::text::truncate_utf8(&req.user_kept, 120),
    );
    let edit = ProfileUpdateEdit {
        edit_event_id: edit_event_id.to_string(),
        recording_id: req.recording_id.clone(),
        run_id: resolved_run_id,
        client_run_id,
        captured_at: Utc::now(),
        raw_transcript: req.raw_transcript.clone(),
        ai_output: req.ai_output.clone(),
        user_kept: req.user_kept.clone(),
        edit_spans,
        target_app: req.target_app.clone(),
        platform: req.platform.clone(),
        output_language: req.output_language.clone(),
        model_used: req.model_used.clone(),
        capture_confidence: req.capture_confidence.clone(),
    };

    let profile_row = current.unwrap_or_else(|| ProfileRow {
        account_id: user.account_id,
        org_scope,
        profile_json: json!({}),
        profile_markdown: String::new(),
        version: from_version,
        schema_version: 1,
        status: "ready".to_string(),
        source_hash: String::new(),
        dirty_at: None,
        last_rebuilt_at: None,
        last_error: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    });

    let update_request =
        build_profile_update_request(user.account_id, org_scope, job_id, edit, &profile_row);
    let update_edit_spans = update_request.edit.edit_spans.len();

    let (job_id, inserted) = enqueue_learn_job(
        &state.db,
        job_id,
        user.account_id,
        org_scope,
        edit_event_id,
        update_request,
        from_version,
    )
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
    })?;

    tracing::info!(
        "[profile] learn-from-edit queued account={} org_scope={} job={} edit_event_id={} idempotent={} from_version={} edit_spans={}",
        user.account_id,
        org_scope,
        job_id,
        edit_event_id,
        !inserted,
        from_version,
        update_edit_spans,
    );

    Ok((
        StatusCode::ACCEPTED,
        Json(LearnFromEditResponse {
            job_id,
            status: "queued",
            message: if inserted {
                None
            } else {
                Some("existing job returned for edit_event_id".into())
            },
        }),
    ))
}

pub async fn approve_profile_proposal(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Path(job_id): Path<Uuid>,
) -> Result<Json<ProfileProposalDecisionResponse>, (StatusCode, Json<Value>)> {
    let org_scope = resolve_scope(&state, &user, &headers).await?;
    let job = get_review_job(&state, user.account_id, org_scope, job_id).await?;
    tracing::info!(
        "[profile] proposal approve requested account={} org_scope={} job={} status={} edit_event={} from_version={}",
        user.account_id,
        org_scope,
        job_id,
        job.status,
        job.edit_event_id,
        job.from_version,
    );
    if job.status != "pending_review" {
        return Err(conflict(format!(
            "proposal is not pending review (status={})",
            job.status
        )));
    }
    let proposal = job
        .response_json
        .clone()
        .ok_or_else(|| conflict("proposal has no reviewed response"))?;
    tracing::info!(
        "[profile] proposal approve loaded account={} org_scope={} job={} terms_delta={} aliases_delta={} first_pass_aliases={} review_required={}",
        user.account_id,
        org_scope,
        job_id,
        proposal
            .get("delta_summary")
            .and_then(|v| v.get("terms_added"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0),
        proposal
            .get("delta_summary")
            .and_then(|v| v.get("aliases_updated"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0),
        proposal
            .get("deepseek")
            .and_then(|v| v.get("alias_proposals"))
            .and_then(|v| v.as_array())
            .map(Vec::len)
            .unwrap_or(0),
        proposal
            .get("review_required")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    );

    let current = store::get_profile(&state.db, user.account_id, org_scope)
        .await
        .map_err(internal_error)?
        .unwrap_or_else(|| ProfileRow {
            account_id: user.account_id,
            org_scope,
            profile_json: json!({}),
            profile_markdown: String::new(),
            version: 0,
            schema_version: 1,
            status: "ready".to_string(),
            source_hash: String::new(),
            dirty_at: None,
            last_rebuilt_at: None,
            last_error: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        });

    if current.version != job.from_version {
        tracing::warn!(
            "[profile] proposal approve stale account={} org_scope={} job={} current_version={} proposal_from_version={}",
            user.account_id,
            org_scope,
            job_id,
            current.version,
            job.from_version,
        );
        return Err(conflict(format!(
            "profile changed since proposal was generated (current={}, proposal={})",
            current.version, job.from_version
        )));
    }

    let mut merged_json = proposal
        .get("merged_profile_json")
        .cloned()
        .ok_or_else(|| conflict("proposal is missing merged_profile_json"))?;
    let mut merged_markdown = proposal
        .get("merged_profile_markdown")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let alias_expansion_audit = expand_aliases_after_approval(
        &state,
        &job,
        &proposal,
        current.version,
        &mut merged_json,
        &mut merged_markdown,
    )
    .await;
    validate_profile_sizes(&merged_json, &merged_markdown)
        .map_err(|e| (StatusCode::BAD_REQUEST, Json(json!({"error": e}))))?;

    let mut audit_patch = proposal
        .get("audit_payload")
        .cloned()
        .unwrap_or_else(|| json!({}));
    if let Some(obj) = audit_patch.as_object_mut() {
        obj.insert("approved_at".to_string(), json!(Utc::now()));
        obj.insert("approved_job_id".to_string(), json!(job_id));
        obj.insert(
            "post_approval_alias_expansion".to_string(),
            alias_expansion_audit,
        );
    }
    let row = store::apply_learned_profile(
        &state.db,
        user.account_id,
        org_scope,
        current.version,
        merged_json,
        merged_markdown,
        proposal
            .get("review_required")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        audit_patch.clone(),
    )
    .await
    .map_err(internal_error)?;

    store::write_profile_audit(
        &state.db,
        user.account_id,
        org_scope,
        current.version,
        row.version,
        "learn_approved",
        audit_patch,
        "api",
    )
    .await
    .map_err(internal_error)?;

    mark_review_job(&state, job_id, "approved", Some(row.version), None).await?;
    invalidate_profile_cache(
        &state,
        &ProfileCacheKey {
            account_id: user.account_id,
            org_scope,
        },
    );

    tracing::info!(
        "[profile] proposal approved account={} org_scope={} job={} edit_event={} v{}→v{} markdown_chars={} aliases_total={} terms_total={}",
        user.account_id,
        org_scope,
        job_id,
        job.edit_event_id,
        current.version,
        row.version,
        row.profile_markdown.chars().count(),
        aliases_from_profile(&row.profile_json).len(),
        stable_terms_from_profile(&row.profile_json).len(),
    );

    Ok(Json(ProfileProposalDecisionResponse {
        job_id,
        status: "approved".to_string(),
        profile_version: Some(row.version),
        message: "profile memory updated".to_string(),
    }))
}

async fn expand_aliases_after_approval(
    state: &AppState,
    job: &ProfileJobReviewRow,
    proposal: &Value,
    profile_version: i64,
    merged_json: &mut Value,
    merged_markdown: &mut String,
) -> Value {
    let alias_request = json!({
        "edit_event_id": job.edit_event_id,
        "job_id": job.id,
        "from_version": job.from_version,
        "approved_profile_patch": proposal.get("deepseek").and_then(|v| v.get("profile_patch")).cloned().unwrap_or_else(|| json!({})),
        "approved_aliases_from_first_pass": proposal.get("deepseek").and_then(|v| v.get("alias_proposals")).cloned().unwrap_or_else(|| json!([])),
        "edit": job.request_json.get("edit").cloned().unwrap_or_else(|| json!({})),
        "current_aliases_after_proposal": merged_json.get("aliases").cloned().unwrap_or_else(|| json!([])),
    });
    tracing::info!(
        "[profile] post-approval alias expansion start job={} edit_event={} profile_version={} current_aliases={} markdown_chars={}",
        job.id,
        job.edit_event_id,
        profile_version,
        merged_json
            .get("aliases")
            .and_then(|v| v.as_array())
            .map(Vec::len)
            .unwrap_or(0),
        merged_markdown.chars().count(),
    );

    let (aliases, reason, latency_ms) =
        match deepseek::call_deepseek_alias_expansion(state, &alias_request).await {
            Ok(result) => result,
            Err(err) => {
                tracing::warn!(
                    "[profile] post-approval alias expansion failed job={}: {err}",
                    job.id
                );
                return json!({
                    "status": "failed_non_blocking",
                    "error": err,
                });
            }
        };

    if aliases.is_empty() {
        tracing::info!(
            "[profile] post-approval alias expansion empty job={} edit_event={} latency_ms={} reason=\"{}\"",
            job.id,
            job.edit_event_id,
            latency_ms,
            said_core::text::truncate_utf8(&reason, 180),
        );
        return json!({
            "status": "empty",
            "latency_ms": latency_ms,
            "reason": reason,
        });
    }

    let deepseek_response = DeepSeekProfileUpdateResponse {
        schema_version: 1,
        classification: DeepSeekClassification::DomainTerm,
        confidence: 0.9,
        profile_patch: DeepSeekProfilePatch::default(),
        alias_proposals: aliases,
        profile_markdown_patch: DeepSeekMarkdownPatch::default(),
        review_required: false,
        reason,
    };

    let output = validate_and_merge(ValidatorInput {
        current_json: merged_json.clone(),
        current_markdown: merged_markdown.clone(),
        current_version: profile_version,
        deepseek: deepseek_response,
        update_mode_apply: true,
        request_id: Uuid::new_v4(),
        edit_event_id: job.edit_event_id.clone(),
        recording_id: job
            .request_json
            .get("edit")
            .and_then(|v| v.get("recording_id"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
        client_run_id: job
            .request_json
            .get("edit")
            .and_then(|v| v.get("client_run_id"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
        run_id: job
            .request_json
            .get("edit")
            .and_then(|v| v.get("run_id"))
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok()),
        latency_ms,
    });

    if matches!(output.decision, ValidatorDecision::Rejected) {
        tracing::info!(
            "[profile] post-approval alias expansion rejected job={} edit_event={} latency_ms={} proposed_aliases={} reasons={}",
            job.id,
            job.edit_event_id,
            latency_ms,
            output.alias_changes.len(),
            output.reasons.join(" | "),
        );
        return json!({
            "status": "rejected_by_validator",
            "latency_ms": latency_ms,
            "reasons": output.reasons,
        });
    }

    *merged_json = output.merged_json;
    *merged_markdown = output.merged_markdown;
    tracing::info!(
        "[profile] post-approval alias expansion merged job={} edit_event={} latency_ms={} alias_changes={} aliases_total={} terms_delta={} aliases_delta={}",
        job.id,
        job.edit_event_id,
        latency_ms,
        output.alias_changes.len(),
        merged_json
            .get("aliases")
            .and_then(|v| v.as_array())
            .map(Vec::len)
            .unwrap_or(0),
        output
            .delta_summary
            .get("terms_added")
            .and_then(|v| v.as_i64())
            .unwrap_or(0),
        output
            .delta_summary
            .get("aliases_updated")
            .and_then(|v| v.as_i64())
            .unwrap_or(0),
    );
    json!({
        "status": "merged",
        "latency_ms": latency_ms,
        "alias_changes": output.alias_changes,
        "delta_summary": output.delta_summary,
    })
}

pub async fn dismiss_profile_proposal(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Path(job_id): Path<Uuid>,
) -> Result<Json<ProfileProposalDecisionResponse>, (StatusCode, Json<Value>)> {
    let org_scope = resolve_scope(&state, &user, &headers).await?;
    let job = get_review_job(&state, user.account_id, org_scope, job_id).await?;
    tracing::info!(
        "[profile] proposal dismiss requested account={} org_scope={} job={} status={} edit_event={} from_version={}",
        user.account_id,
        org_scope,
        job_id,
        job.status,
        job.edit_event_id,
        job.from_version,
    );
    if job.status != "pending_review" {
        return Err(conflict(format!(
            "proposal is not pending review (status={})",
            job.status
        )));
    }
    store::write_profile_audit(
        &state.db,
        user.account_id,
        org_scope,
        job.from_version,
        job.from_version,
        "learn_dismissed",
        json!({
            "job_id": job_id,
            "edit_event_id": job.edit_event_id,
            "dismissed_at": Utc::now(),
        }),
        "api",
    )
    .await
    .map_err(internal_error)?;
    mark_review_job(&state, job_id, "dismissed", None, None).await?;
    tracing::info!(
        "[profile] proposal dismissed account={} org_scope={} job={} edit_event={} from_version={}",
        user.account_id,
        org_scope,
        job_id,
        job.edit_event_id,
        job.from_version,
    );
    Ok(Json(ProfileProposalDecisionResponse {
        job_id,
        status: "dismissed".to_string(),
        profile_version: None,
        message: "proposal dismissed".to_string(),
    }))
}

async fn list_pending_proposals(
    state: &AppState,
    account_id: Uuid,
    org_scope: Uuid,
) -> Result<Vec<ProfileLearningProposal>, (StatusCode, Json<Value>)> {
    let current_version = store::get_profile(&state.db, account_id, org_scope)
        .await
        .map_err(internal_error)?
        .map(|row| row.version)
        .unwrap_or(0);
    let rows = sqlx::query_as::<_, ProfileJobReviewRow>(
        "SELECT id, edit_event_id, status, request_json, response_json, from_version,
                to_version, error, created_at, updated_at
           FROM runtime_profile_learn_jobs
          WHERE account_id = $1
            AND org_scope = $2
            AND from_version = $3
            AND status = 'pending_review'
          ORDER BY updated_at DESC
          LIMIT 20",
    )
    .bind(account_id)
    .bind(org_scope)
    .bind(current_version)
    .fetch_all(&state.db)
    .await
    .map_err(internal_error)?;

    Ok(rows.into_iter().map(proposal_from_job).collect())
}

async fn get_review_job(
    state: &AppState,
    account_id: Uuid,
    org_scope: Uuid,
    job_id: Uuid,
) -> Result<ProfileJobReviewRow, (StatusCode, Json<Value>)> {
    sqlx::query_as::<_, ProfileJobReviewRow>(
        "SELECT id, edit_event_id, status, request_json, response_json, from_version,
                to_version, error, created_at, updated_at
           FROM runtime_profile_learn_jobs
          WHERE id = $1
            AND account_id = $2
            AND org_scope = $3",
    )
    .bind(job_id)
    .bind(account_id)
    .bind(org_scope)
    .fetch_optional(&state.db)
    .await
    .map_err(internal_error)?
    .ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "proposal not found"})),
        )
    })
}

async fn mark_review_job(
    state: &AppState,
    job_id: Uuid,
    status: &str,
    to_version: Option<i64>,
    error: Option<&str>,
) -> Result<(), (StatusCode, Json<Value>)> {
    sqlx::query(
        "UPDATE runtime_profile_learn_jobs
            SET status = $2,
                to_version = $3,
                error = $4,
                updated_at = now()
          WHERE id = $1",
    )
    .bind(job_id)
    .bind(status)
    .bind(to_version)
    .bind(error)
    .execute(&state.db)
    .await
    .map_err(internal_error)?;
    Ok(())
}

fn proposal_from_job(job: ProfileJobReviewRow) -> ProfileLearningProposal {
    let edit = job
        .request_json
        .get("edit")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let response = job.response_json.unwrap_or_else(|| json!({}));
    let deepseek = response
        .get("deepseek")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let patch = deepseek
        .get("profile_patch")
        .cloned()
        .unwrap_or_else(|| json!({}));

    ProfileLearningProposal {
        job_id: job.id,
        edit_event_id: job.edit_event_id,
        status: job.status,
        from_version: job.from_version,
        created_at: job.created_at,
        updated_at: job.updated_at,
        ai_output: edit
            .get("ai_output")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        user_kept: edit
            .get("user_kept")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        raw_transcript: edit
            .get("raw_transcript")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        classification: deepseek
            .get("classification")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        confidence: deepseek.get("confidence").and_then(|v| v.as_f64()),
        reason: deepseek
            .get("reason")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        delta_summary: response
            .get("delta_summary")
            .cloned()
            .unwrap_or_else(|| json!({})),
        stable_terms: stable_terms_from_patch(&patch),
        aliases: aliases_from_deepseek(&deepseek),
        domains: domains_from_patch(&patch),
    }
}

fn stable_terms_from_profile(profile_json: &Value) -> Vec<ProfileMemoryTerm> {
    profile_json
        .get("stable_terms")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    Some(ProfileMemoryTerm {
                        term: item.get("term")?.as_str()?.to_string(),
                        term_type: item
                            .get("term_type")
                            .and_then(|v| v.as_str())
                            .map(str::to_string),
                        evidence: item
                            .get("evidence")
                            .and_then(|v| v.as_str())
                            .map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn stable_terms_from_patch(patch: &Value) -> Vec<ProfileMemoryTerm> {
    patch
        .get("add_stable_terms")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    Some(ProfileMemoryTerm {
                        term: item.get("term")?.as_str()?.to_string(),
                        term_type: item
                            .get("term_type")
                            .and_then(|v| v.as_str())
                            .map(str::to_string),
                        evidence: item
                            .get("evidence")
                            .and_then(|v| v.as_str())
                            .map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn aliases_from_profile(profile_json: &Value) -> Vec<ProfileMemoryAlias> {
    profile_json
        .get("aliases")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    Some(ProfileMemoryAlias {
                        source_phrase: item.get("source_phrase")?.as_str()?.to_string(),
                        canonical_phrase: item.get("canonical_phrase")?.as_str()?.to_string(),
                        status: item
                            .get("status")
                            .and_then(|v| v.as_str())
                            .unwrap_or("candidate")
                            .to_string(),
                        confidence: item.get("confidence").and_then(|v| v.as_f64()),
                        evidence_count: item
                            .get("evidence_count")
                            .and_then(|v| v.as_i64())
                            .map(|v| v as i32),
                        reason: item
                            .get("reason")
                            .and_then(|v| v.as_str())
                            .map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn aliases_from_deepseek(deepseek: &Value) -> Vec<ProfileMemoryAlias> {
    deepseek
        .get("alias_proposals")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    Some(ProfileMemoryAlias {
                        source_phrase: item.get("source_phrase")?.as_str()?.to_string(),
                        canonical_phrase: item.get("canonical_phrase")?.as_str()?.to_string(),
                        status: item
                            .get("proposal_status")
                            .and_then(|v| v.as_str())
                            .unwrap_or("candidate")
                            .to_string(),
                        confidence: item.get("confidence").and_then(|v| v.as_f64()),
                        evidence_count: item
                            .get("evidence_count_delta")
                            .and_then(|v| v.as_i64())
                            .map(|v| v as i32),
                        reason: item
                            .get("reason")
                            .and_then(|v| v.as_str())
                            .map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn domains_from_profile(profile_json: &Value) -> Vec<ProfileMemoryDomain> {
    profile_json
        .get("domains")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    Some(ProfileMemoryDomain {
                        name: item.get("name")?.as_str()?.to_string(),
                        weight: item.get("weight").and_then(|v| v.as_f64()),
                        evidence: item
                            .get("evidence")
                            .and_then(|v| v.as_str())
                            .map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn domains_from_patch(patch: &Value) -> Vec<ProfileMemoryDomain> {
    patch
        .get("add_domains")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    Some(ProfileMemoryDomain {
                        name: item.get("name")?.as_str()?.to_string(),
                        weight: item.get("weight").and_then(|v| v.as_f64()),
                        evidence: item
                            .get("evidence")
                            .and_then(|v| v.as_str())
                            .map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Shadow/live profile load for runtime polish. Returns cache hit flag.
pub async fn load_profile_for_polish(
    state: &AppState,
    account_id: Uuid,
    org_scope: Uuid,
) -> (Option<CachedRuntimeProfile>, bool) {
    let key = ProfileCacheKey {
        account_id,
        org_scope,
    };
    if let Some(hit) = state.profile_cache.get(&key) {
        return (Some(hit), true);
    }
    match profile::load_profile_cached_for_scope(state, account_id, org_scope).await {
        Ok(profile) => (profile, false),
        Err(err) => {
            tracing::warn!(
                "[profile] load failed account={} org_scope={}: {err}",
                account_id,
                org_scope
            );
            (None, false)
        }
    }
}

pub fn profile_markdown_for_prompt(profile: Option<&CachedRuntimeProfile>) -> Option<&str> {
    profile.and_then(|p| {
        if p.profile_markdown.trim().is_empty() {
            None
        } else {
            Some(p.profile_markdown.as_str())
        }
    })
}

pub fn log_profile_shadow(profile: Option<&CachedRuntimeProfile>, cache_hit: bool) {
    let (version, schema_version, status, source_hash, chars) = match profile {
        Some(p) => (
            p.version,
            p.schema_version,
            p.status.as_str(),
            p.source_hash.as_str(),
            p.sanitized_markdown().len(),
        ),
        None => (0, 1, "missing", "", 0),
    };
    tracing::info!(
        "[profile] prompt account profile_version={} profile_schema_version={} profile_status={} profile_source_hash={} profile_cache_hit={} profile_chars={}",
        version,
        schema_version,
        status,
        source_hash,
        cache_hit,
        chars,
    );
    if let Some(p) = profile {
        let sanitized = p.sanitized_markdown();
        if !sanitized.is_empty() {
            tracing::debug!(
                "[profile] prompt block preview chars={}: {}",
                sanitized.len(),
                said_core::text::truncate_utf8(&sanitized, 400),
            );
        }
    }
}
