//! First-launch local → server data migration.
//!
//! Routes:
//!   GET  /v1/server-migration/status  — current state and counts
//!   POST /v1/server-migration/run     — start or retry migration in background
//!   POST /v1/server-migration/cancel  — cancel in-progress attempt

use axum::{Json, extract::State, http::StatusCode};
use serde::Serialize;
use serde_json::{Value, json};
use tracing::{info, warn};

use crate::{
    AppState,
    store::{
        self, email_memory, history as history_store, server_migration as mig_store,
        server_migration::MigrationStatus, stt_replacements, users, vocabulary,
    },
};

const MIGRATION_VERSION: i64 = 1;

// ── Response types ────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct MigrationStatusResponse {
    pub status: String,
    pub migration_version: i64,
    pub uploaded_history_count: i64,
    pub uploaded_vocab_count: i64,
    pub uploaded_alias_count: i64,
    pub uploaded_email_count: i64,
    pub uploaded_credentials_count: i64,
    pub last_error: Option<String>,
    pub last_attempt_at_ms: Option<i64>,
    pub completed_at_ms: Option<i64>,
    pub server_url: Option<String>,
    pub signed_in: bool,
}

// ── GET /v1/server-migration/status ──────────────────────────────────────────

pub async fn status(State(state): State<AppState>) -> Json<MigrationStatusResponse> {
    let (user_id, server_account_id, server_url, signed_in) = resolve_ids(&state);

    let row = mig_store::get_state(&state.pool, &user_id, &server_account_id);

    Json(MigrationStatusResponse {
        status: row
            .as_ref()
            .map(|r| r.status.as_str().to_string())
            .unwrap_or_else(|| "not_started".to_string()),
        migration_version: row
            .as_ref()
            .map(|r| r.migration_version)
            .unwrap_or(MIGRATION_VERSION),
        uploaded_history_count: row.as_ref().map(|r| r.uploaded_history_count).unwrap_or(0),
        uploaded_vocab_count: row.as_ref().map(|r| r.uploaded_vocab_count).unwrap_or(0),
        uploaded_alias_count: row.as_ref().map(|r| r.uploaded_alias_count).unwrap_or(0),
        uploaded_email_count: row.as_ref().map(|r| r.uploaded_email_count).unwrap_or(0),
        uploaded_credentials_count: row
            .as_ref()
            .map(|r| r.uploaded_credentials_count)
            .unwrap_or(0),
        last_error: row.as_ref().and_then(|r| r.last_error.clone()),
        last_attempt_at_ms: row.as_ref().and_then(|r| r.last_attempt_at_ms),
        completed_at_ms: row.as_ref().and_then(|r| r.completed_at_ms),
        server_url,
        signed_in,
    })
}

// ── POST /v1/server-migration/run ─────────────────────────────────────────────

pub async fn run(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    let (user_id, server_account_id, server_url, signed_in) = resolve_ids(&state);

    if !signed_in {
        return (
            StatusCode::PRECONDITION_FAILED,
            Json(json!({"started": false, "reason": "not signed in"})),
        );
    }

    let Some(url) = server_url else {
        return (
            StatusCode::PRECONDITION_FAILED,
            Json(json!({"started": false, "reason": "server URL not configured"})),
        );
    };

    // Check current state — don't double-start
    mig_store::ensure_row(&state.pool, &user_id, &server_account_id, MIGRATION_VERSION);
    let row = mig_store::get_state(&state.pool, &user_id, &server_account_id);
    if let Some(ref r) = row {
        if r.status == MigrationStatus::Running {
            return (
                StatusCode::ACCEPTED,
                Json(json!({"started": false, "reason": "already running"})),
            );
        }
        if r.status == MigrationStatus::Completed {
            return (
                StatusCode::OK,
                Json(json!({"started": false, "reason": "already completed"})),
            );
        }
    }

    mig_store::set_status(
        &state.pool,
        &user_id,
        &server_account_id,
        MIGRATION_VERSION,
        MigrationStatus::Running,
        None,
    );

    // Collect token and clone everything for the background task
    let token = users::get_user(&state.pool, &state.default_user_id)
        .and_then(|u| u.cloud_token)
        .unwrap_or_default();

    let pool = state.pool.clone();
    let http = state.http_client.clone();
    let uid = user_id.clone();
    let said = server_account_id.clone();

    tokio::spawn(async move {
        run_migration_task(pool, http, uid, said, url, token, MIGRATION_VERSION).await;
    });

    (StatusCode::ACCEPTED, Json(json!({"started": true})))
}

// ── POST /v1/server-migration/cancel ─────────────────────────────────────────

pub async fn cancel(State(state): State<AppState>) -> Json<Value> {
    let (user_id, server_account_id, _, _) = resolve_ids(&state);

    let row = mig_store::get_state(&state.pool, &user_id, &server_account_id);
    if matches!(
        row.as_ref().map(|r| &r.status),
        Some(MigrationStatus::Running)
    ) {
        mig_store::set_status(
            &state.pool,
            &user_id,
            &server_account_id,
            MIGRATION_VERSION,
            MigrationStatus::Partial,
            Some("cancelled by user"),
        );
        Json(json!({"cancelled": true}))
    } else {
        Json(json!({"cancelled": false, "reason": "not running"}))
    }
}

// ── Background migration task ─────────────────────────────────────────────────

async fn run_migration_task(
    pool: store::DbPool,
    http: reqwest::Client,
    user_id: String,
    server_account_id: String,
    server_url: String,
    token: String,
    version: i64,
) {
    info!("[server-migration] starting version={version}");

    let base = server_url.trim_end_matches('/');
    let mut failed = false;

    // Step 1: Sync credentials
    // The sync route posts each provider key from local prefs to the server vault.
    // We re-use the existing credential sync helper.
    let cred_count = upload_credentials(&pool, &http, base, &token, &user_id).await;
    mig_store::update_counts(
        &pool,
        &user_id,
        &server_account_id,
        version,
        0,
        0,
        0,
        0,
        cred_count,
    );

    // Step 2: Upload history
    let history_accepted = upload_history(&pool, &http, base, &token).await;
    if history_accepted < 0 {
        failed = true;
    }
    mig_store::update_counts(
        &pool,
        &user_id,
        &server_account_id,
        version,
        history_accepted.max(0),
        0,
        0,
        0,
        0,
    );

    // Step 3: Upload memory (vocab + aliases + emails)
    let (vocab_n, alias_n, email_n) = upload_memory(&pool, &http, base, &token).await;
    mig_store::update_counts(
        &pool,
        &user_id,
        &server_account_id,
        version,
        0,
        vocab_n,
        alias_n,
        email_n,
        0,
    );

    let final_status = if failed {
        MigrationStatus::Partial
    } else {
        MigrationStatus::Completed
    };

    mig_store::set_status(
        &pool,
        &user_id,
        &server_account_id,
        version,
        final_status,
        None,
    );
    info!("[server-migration] done version={version} failed={failed}");
}

async fn upload_credentials(
    pool: &store::DbPool,
    http: &reqwest::Client,
    base: &str,
    token: &str,
    user_id: &str,
) -> i64 {
    let url = format!("{base}/v1/runtime/credentials");
    let Some(prefs) = crate::store::prefs::get_prefs(pool, user_id) else {
        return 0;
    };
    let secrets: Vec<(&str, &str, String)> = {
        let mut v: Vec<(&str, &str, String)> = Vec::new();
        if let Some(k) = prefs
            .groq_api_key
            .as_deref()
            .filter(|s| !s.trim().is_empty())
        {
            v.push(("groq", "Groq API key", k.to_string()));
        }
        if let Some(k) = prefs
            .gateway_api_key
            .as_deref()
            .filter(|s| !s.trim().is_empty())
        {
            v.push(("gateway", "Gateway API key", k.to_string()));
        }
        if let Some(k) = prefs
            .gemini_api_key
            .as_deref()
            .filter(|s| !s.trim().is_empty())
        {
            v.push(("gemini", "Gemini API key", k.to_string()));
        }
        v
    };
    let mut synced = 0i64;
    for (provider, display, secret) in &secrets {
        let body = json!({ "provider": provider, "scope": "user", "display_name": display, "secret": secret });
        let res = http
            .post(&url)
            .bearer_auth(token)
            .json(&body)
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await;
        if res.map(|r| r.status().is_success()).unwrap_or(false) {
            synced += 1;
        }
    }
    synced
}

async fn upload_history(
    pool: &store::DbPool,
    http: &reqwest::Client,
    base: &str,
    token: &str,
) -> i64 {
    let url = format!("{base}/v1/runtime/history/sync");
    let mut before_ms = None;
    let mut total_accepted = 0i64;

    loop {
        let recordings = history_store::list_recordings(pool, "default", 500, before_ms);
        if recordings.is_empty() {
            break;
        }

        before_ms = recordings.iter().map(|r| r.timestamp_ms).min();

        let items: Vec<Value> = recordings
            .iter()
            .map(|r| {
                let created_at =
                    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(r.timestamp_ms)
                        .map(|ts| ts.to_rfc3339());
                json!({
                    "recording_id": r.id,
                    "source": r.source,
                    "transcript": r.transcript,
                    "polished_output": r.polished,
                    "final_text": r.final_text,
                    "model_used": r.model_used,
                    "word_count": r.word_count,
                    "recording_seconds": r.recording_seconds,
                    "transcribe_ms": r.transcribe_ms,
                    "embed_ms": r.embed_ms,
                    "polish_ms": r.polish_ms,
                    "target_app": r.target_app,
                    "raw_transcript": r.raw_transcript,
                    "local_corrected_transcript": r.local_corrected_transcript,
                    "created_at": created_at,
                })
            })
            .collect();

        let body = json!({ "items": items });
        let res = http
            .post(&url)
            .bearer_auth(token)
            .json(&body)
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await;

        match res {
            Ok(r) if r.status().is_success() => {
                total_accepted += r
                    .json::<Value>()
                    .await
                    .ok()
                    .and_then(|v| v.get("accepted").and_then(Value::as_i64))
                    .unwrap_or(recordings.len() as i64);
            }
            Ok(r) => {
                warn!(
                    "[server-migration] history upload failed status={}",
                    r.status()
                );
                return -1;
            }
            Err(e) => {
                warn!("[server-migration] history upload error: {e}");
                return -1;
            }
        }

        if recordings.len() < 500 {
            break;
        }
    }

    total_accepted
}

async fn upload_memory(
    pool: &store::DbPool,
    http: &reqwest::Client,
    base: &str,
    token: &str,
) -> (i64, i64, i64) {
    let url = format!("{base}/v1/runtime/memory/sync");

    let terms = vocabulary::top_terms(pool, "default", 500);
    let aliases = stt_replacements::load_all(pool, "default");
    let emails = email_memory::load_candidates(pool, "default");

    let vocab_items: Vec<Value> = terms
        .iter()
        .map(|t| {
            json!({
                "term": t.term,
                "term_type": t.term_type,
                "weight": t.weight,
            })
        })
        .collect();

    let alias_items: Vec<Value> = aliases
        .iter()
        .filter(|a| {
            use stt_replacements::ExportTier;
            !matches!(a.export_tier, ExportTier::Blocked)
        })
        .map(|a| {
            json!({
                "transcript_form": a.transcript_form,
                "correct_form": a.correct_form,
                "edit_type": "replace",
            })
        })
        .collect();

    let email_items: Vec<Value> = emails.iter().map(|e| json!({ "email": e })).collect();

    let body = json!({
        "vocab_terms": vocab_items,
        "stt_replacements": alias_items,
        "email_memory": email_items,
    });

    let res = http
        .post(&url)
        .bearer_auth(token)
        .json(&body)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await;

    match res {
        Ok(r) if r.status().is_success() => {
            let v = r.json::<Value>().await.unwrap_or_default();
            let vn = v
                .get("accepted_vocab")
                .and_then(Value::as_i64)
                .unwrap_or(vocab_items.len() as i64);
            let an = v
                .get("accepted_aliases")
                .and_then(Value::as_i64)
                .unwrap_or(alias_items.len() as i64);
            let en = v
                .get("accepted_emails")
                .and_then(Value::as_i64)
                .unwrap_or(email_items.len() as i64);
            (vn, an, en)
        }
        Ok(r) => {
            warn!(
                "[server-migration] memory upload failed status={}",
                r.status()
            );
            (0, 0, 0)
        }
        Err(e) => {
            warn!("[server-migration] memory upload error: {e}");
            (0, 0, 0)
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn resolve_ids(state: &AppState) -> (String, String, Option<String>, bool) {
    let user = users::get_user(&state.pool, &state.default_user_id);
    let signed_in = user
        .as_ref()
        .and_then(|u| u.cloud_token.as_deref())
        .map(|t| !t.trim().is_empty())
        .unwrap_or(false);
    // Use the user's email as a stable server-account identifier.
    let server_account_id = user
        .as_ref()
        .map(|u| u.email.clone())
        .filter(|e| !e.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    let server_url = user
        .as_ref()
        .and_then(|u| u.enterprise_server_url.as_deref())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("AIRNOTE_CONTROL_PLANE_URL").ok())
        .or_else(|| std::env::var("CLOUD_API_URL").ok());

    (
        state.default_user_id.to_string(),
        server_account_id,
        server_url,
        signed_in,
    )
}
