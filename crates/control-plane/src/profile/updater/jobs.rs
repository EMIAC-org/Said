//! Learn-from-edit job enqueue + processing.

use chrono::Utc;
use serde_json::{Value, json};
use sqlx::PgPool;
use tracing::{info, warn};
use uuid::Uuid;

use crate::AppState;
use crate::profile::{
    store::{self, PROFILE_JSON_MAX_BYTES, PROFILE_MARKDOWN_MAX_BYTES, ProfileRow},
    updater::{
        deepseek,
        types::{
            AliasSummaryEntry, LearnJobRow, ProfileUpdateCurrentProfile, ProfileUpdateEdit,
            ProfileUpdatePolicy, ProfileUpdateRequest,
        },
        validator::{ValidatorDecision, ValidatorInput, validate_and_merge},
    },
};

pub async fn enqueue_learn_job(
    db: &PgPool,
    job_id: Uuid,
    account_id: Uuid,
    org_scope: Uuid,
    edit_event_id: &str,
    request: ProfileUpdateRequest,
    from_version: i64,
) -> Result<(Uuid, bool), sqlx::Error> {
    let request_json = serde_json::to_value(&request).unwrap_or_else(|_| json!({}));

    let inserted = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO runtime_profile_learn_jobs
            (id, account_id, org_scope, edit_event_id, status, request_json, from_version)
         VALUES ($1, $2, $3, $4, 'queued', $5, $6)
         ON CONFLICT (account_id, org_scope, edit_event_id) DO NOTHING
         RETURNING id",
    )
    .bind(job_id)
    .bind(account_id)
    .bind(org_scope)
    .bind(edit_event_id)
    .bind(&request_json)
    .bind(from_version)
    .fetch_optional(db)
    .await?;

    if let Some(existing_id) = inserted {
        return Ok((existing_id, true));
    }

    let existing: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM runtime_profile_learn_jobs
          WHERE account_id = $1 AND org_scope = $2 AND edit_event_id = $3",
    )
    .bind(account_id)
    .bind(org_scope)
    .bind(edit_event_id)
    .fetch_optional(db)
    .await?;

    Ok((existing.unwrap_or(job_id), false))
}

pub fn build_profile_update_request(
    account_id: Uuid,
    org_scope: Uuid,
    job_id: Uuid,
    edit: ProfileUpdateEdit,
    current: &ProfileRow,
) -> ProfileUpdateRequest {
    let alias_summary = alias_summary_from_json(&current.profile_json);
    let profile_json = truncate_profile_json_for_send(&current.profile_json);
    let profile_markdown = current
        .profile_markdown
        .chars()
        .take(PROFILE_MARKDOWN_MAX_BYTES)
        .collect();

    ProfileUpdateRequest {
        schema_version: 1,
        request_id: job_id,
        account_id,
        org_scope,
        edit,
        current_profile: ProfileUpdateCurrentProfile {
            version: current.version,
            schema_version: current.schema_version,
            profile_json,
            profile_markdown,
            alias_summary,
        },
        policy: ProfileUpdatePolicy {
            max_markdown_bytes: PROFILE_MARKDOWN_MAX_BYTES,
            max_json_bytes: PROFILE_JSON_MAX_BYTES,
            alias_min_confidence_candidate: 0.55,
            alias_min_confidence_active: 0.82,
            alias_min_evidence_active: 2,
        },
    }
}

fn alias_summary_from_json(profile_json: &Value) -> Vec<AliasSummaryEntry> {
    profile_json
        .get("aliases")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|e| {
                    Some(AliasSummaryEntry {
                        source_phrase: e.get("source_phrase")?.as_str()?.to_string(),
                        canonical_phrase: e.get("canonical_phrase")?.as_str()?.to_string(),
                        status: e
                            .get("status")
                            .and_then(|s| s.as_str())
                            .unwrap_or("candidate")
                            .to_string(),
                        evidence_count: e
                            .get("evidence_count")
                            .and_then(|c| c.as_i64())
                            .unwrap_or(0) as i32,
                    })
                })
                .take(32)
                .collect()
        })
        .unwrap_or_default()
}

fn truncate_profile_json_for_send(profile_json: &Value) -> Value {
    let mut copy = profile_json.clone();
    if let Some(arr) = copy
        .get_mut("recent_context")
        .and_then(|v| v.as_array_mut())
    {
        if arr.len() > 5 {
            arr.drain(0..arr.len() - 5);
        }
    }
    let bytes = serde_json::to_vec(&copy).unwrap_or_default();
    if bytes.len() <= PROFILE_JSON_MAX_BYTES {
        return copy;
    }
    copy.as_object_mut().map(|o| {
        o.remove("recent_context");
    });
    copy
}

pub async fn process_learn_job(state: &AppState, job_id: Uuid) -> Result<(), String> {
    let job = sqlx::query_as::<_, LearnJobRow>(
        "SELECT id, account_id, org_scope, edit_event_id, status, request_json,
                response_json, from_version, to_version, error
           FROM runtime_profile_learn_jobs
          WHERE id = $1",
    )
    .bind(job_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| e.to_string())?
    .ok_or_else(|| "job not found".to_string())?;

    if job.status != "queued" && job.status != "processing" {
        return Ok(());
    }

    sqlx::query(
        "UPDATE runtime_profile_learn_jobs SET status = 'processing', updated_at = now() WHERE id = $1",
    )
    .bind(job_id)
    .execute(&state.db)
    .await
    .map_err(|e| e.to_string())?;

    let request: ProfileUpdateRequest = serde_json::from_value(job.request_json.clone())
        .map_err(|e| format!("invalid stored request: {e}"))?;
    info!(
        "[profile-updater] job processing start job={} account={} org_scope={} edit_event={} from_version={} ai_chars={} kept_chars={} raw_chars={} edit_spans={} client_run_id={} run_id={} ai_preview=\"{}\" kept_preview=\"{}\"",
        job_id,
        job.account_id,
        job.org_scope,
        job.edit_event_id,
        job.from_version,
        request.edit.ai_output.chars().count(),
        request.edit.user_kept.chars().count(),
        request
            .edit
            .raw_transcript
            .as_deref()
            .map(|s| s.chars().count())
            .unwrap_or(0),
        request.edit.edit_spans.len(),
        request.edit.client_run_id.as_deref().unwrap_or("none"),
        request
            .edit
            .run_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| "none".to_string()),
        said_core::text::truncate_utf8(&request.edit.ai_output, 120),
        said_core::text::truncate_utf8(&request.edit.user_kept, 120),
    );

    let deepseek_result = deepseek::call_deepseek_profile_update(state, &request).await;

    match deepseek_result {
        Ok((response, latency_ms)) => {
            let response_json = serde_json::to_value(&response).unwrap_or_else(|_| json!({}));
            let _ = sqlx::query(
                "UPDATE runtime_profile_learn_jobs SET response_json = $2, updated_at = now() WHERE id = $1",
            )
            .bind(job_id)
            .bind(&response_json)
            .execute(&state.db)
            .await;

            finish_with_validator(
                state,
                &job,
                job_id,
                request.request_id,
                request.edit,
                response,
                latency_ms,
            )
            .await
        }
        Err(err) => {
            fail_job(state, &job, job_id, &err).await;
            Err(err)
        }
    }
}

async fn finish_with_validator(
    state: &AppState,
    job: &LearnJobRow,
    job_id: Uuid,
    request_id: Uuid,
    edit: crate::profile::updater::types::ProfileUpdateEdit,
    deepseek_response: crate::profile::updater::types::DeepSeekProfileUpdateResponse,
    latency_ms: u64,
) -> Result<(), String> {
    let current = store::get_profile(&state.db, job.account_id, job.org_scope)
        .await
        .map_err(|e| e.to_string())?
        .unwrap_or_else(|| ProfileRow {
            account_id: job.account_id,
            org_scope: job.org_scope,
            profile_json: json!({}),
            profile_markdown: String::new(),
            version: job.from_version,
            schema_version: 1,
            status: "ready".to_string(),
            source_hash: String::new(),
            dirty_at: None,
            last_rebuilt_at: None,
            last_error: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        });

    let deepseek_for_payload = deepseek_response.clone();
    let output = validate_and_merge(ValidatorInput {
        current_json: current.profile_json.clone(),
        current_markdown: current.profile_markdown.clone(),
        current_version: current.version,
        deepseek: deepseek_response,
        update_mode_apply: true,
        request_id,
        edit_event_id: job.edit_event_id.clone(),
        recording_id: edit.recording_id.clone(),
        client_run_id: edit.client_run_id.clone(),
        run_id: edit.run_id,
        latency_ms,
    });
    info!(
        "[profile-updater] validator complete job={} account={} org_scope={} edit_event={} decision={:?} review_required={} profile_sections_delta={} terms_delta={} aliases_delta={} alias_changes={} reasons={}",
        job_id,
        job.account_id,
        job.org_scope,
        job.edit_event_id,
        output.decision,
        output.review_required,
        output
            .delta_summary
            .get("profile_sections_updated")
            .and_then(|v| v.as_i64())
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
        output.alias_changes.len(),
        if output.reasons.is_empty() {
            "none".to_string()
        } else {
            output.reasons.join(" | ")
        },
    );

    let mut audit_payload = output.audit_payload.clone();
    audit_payload.job_id = Some(job_id);
    if !matches!(output.decision, ValidatorDecision::Rejected) {
        audit_payload.validator_decision = Some("pending_review".to_string());
    }
    let audit_value = serde_json::to_value(&audit_payload).unwrap_or_else(|_| json!({}));

    match output.decision {
        ValidatorDecision::Applied | ValidatorDecision::Shadow => {
            let proposal_json = json!({
                "deepseek": deepseek_for_payload,
                "audit_payload": audit_payload,
                "merged_profile_json": output.merged_json,
                "merged_profile_markdown": output.merged_markdown,
                "review_required": output.review_required,
                "validator_reasons": output.reasons,
                "alias_changes": output.alias_changes,
                "delta_summary": output.delta_summary,
                "proposal_created_at": Utc::now(),
                "from_version": current.version,
            });

            store::write_profile_audit(
                &state.db,
                job.account_id,
                job.org_scope,
                current.version,
                current.version,
                "learn_proposed",
                audit_value,
                "validator",
            )
            .await
            .map_err(|e| e.to_string())?;

            mark_job_pending_review(&state.db, job_id, &proposal_json).await?;
            info!(
                "[profile-updater] pending_review created job={} account={} org_scope={} edit_event={} from_version={} profile_sections_delta={} terms_delta={} aliases_delta={} review_required={}",
                job_id,
                job.account_id,
                job.org_scope,
                job.edit_event_id,
                current.version,
                proposal_json
                    .get("delta_summary")
                    .and_then(|v| v.get("profile_sections_updated"))
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0),
                proposal_json
                    .get("delta_summary")
                    .and_then(|v| v.get("terms_added"))
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0),
                proposal_json
                    .get("delta_summary")
                    .and_then(|v| v.get("aliases_updated"))
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0),
                proposal_json
                    .get("review_required")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
            );
        }
        ValidatorDecision::Rejected => {
            store::write_profile_audit(
                &state.db,
                job.account_id,
                job.org_scope,
                current.version,
                current.version,
                "learn_rejected",
                audit_value,
                "validator",
            )
            .await
            .map_err(|e| e.to_string())?;

            mark_job_terminal(&state.db, job_id, "rejected", None, None).await?;
            info!(
                "[profile-updater] rejected job={} account={} reasons={:?}",
                job_id, job.account_id, output.reasons
            );
        }
    }

    Ok(())
}

async fn mark_job_pending_review(
    db: &PgPool,
    job_id: Uuid,
    response_json: &Value,
) -> Result<(), String> {
    sqlx::query(
        "UPDATE runtime_profile_learn_jobs
            SET status = 'pending_review',
                response_json = $2,
                error = NULL,
                processed_at = now(),
                updated_at = now()
          WHERE id = $1",
    )
    .bind(job_id)
    .bind(response_json)
    .execute(db)
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

async fn fail_job(state: &AppState, job: &LearnJobRow, job_id: Uuid, err: &str) {
    let (client_run_id, run_id) = job
        .request_json
        .get("edit")
        .map(|edit| {
            let client_run_id = edit
                .get("client_run_id")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let run_id = edit
                .get("run_id")
                .and_then(|v| v.as_str())
                .and_then(|s| Uuid::parse_str(s).ok());
            (client_run_id, run_id)
        })
        .unwrap_or((None, None));

    let audit = json!({
        "edit_event_id": job.edit_event_id,
        "job_id": job_id,
        "client_run_id": client_run_id,
        "run_id": run_id,
        "error": err,
    });
    let _ = store::write_profile_audit(
        &state.db,
        job.account_id,
        job.org_scope,
        job.from_version,
        job.from_version,
        "learn_failed",
        audit,
        "deepseek_edit",
    )
    .await;

    let _ = mark_job_terminal(&state.db, job_id, "failed", None, Some(err)).await;
    warn!(
        "[profile-updater] failed job={} account={}: {err}",
        job_id, job.account_id
    );
}

async fn mark_job_terminal(
    db: &PgPool,
    job_id: Uuid,
    status: &str,
    to_version: Option<i64>,
    error: Option<&str>,
) -> Result<(), String> {
    sqlx::query(
        "UPDATE runtime_profile_learn_jobs
            SET status = $2, to_version = $3, error = $4, processed_at = now(), updated_at = now()
          WHERE id = $1",
    )
    .bind(job_id)
    .bind(status)
    .bind(to_version)
    .bind(error)
    .execute(db)
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}
