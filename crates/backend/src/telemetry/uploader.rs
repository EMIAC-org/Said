//! Background batch uploader — desktop SQLite outbox → control plane.

use tracing::{debug, info, warn};

use crate::{
    cp_client,
    store::{
        DbPool,
        telemetry::{self, DailyRollupRow, RunSummaryRow},
        users,
    },
};

const BATCH_RUN_LIMIT: usize = 50;
const BATCH_ROLLUP_LIMIT: usize = 14;

#[derive(serde::Serialize)]
struct TelemetryBatchRequest {
    run_summaries: Vec<RunSummaryPayload>,
    daily_rollups: Vec<DailyRollupPayload>,
    client_version: String,
    device_id: String,
    sent_at: i64,
}

#[derive(serde::Serialize)]
struct RunSummaryPayload {
    run_id: String,
    recording_id: Option<String>,
    device_id: Option<String>,
    mode: String,
    target_app: Option<String>,
    platform: Option<String>,
    app_version: Option<String>,
    machine_class: Option<String>,
    audio_seconds: Option<f64>,
    word_count: Option<i32>,
    char_count: Option<i32>,
    transcribe_ms: Option<i32>,
    embed_ms: Option<i32>,
    polish_ms: Option<i32>,
    total_ms: Option<i32>,
    paste_ms: Option<i32>,
    success: bool,
    error_code: Option<String>,
    used_clipboard_fallback: bool,
    used_ws_pretranscript: bool,
    used_http_stt_fallback: bool,
    stt_provider: Option<String>,
    stt_model: Option<String>,
    stt_path: Option<String>,
    edit_detected: bool,
    edit_bucket: String,
    edit_distance_chars: Option<i32>,
    edit_distance_words: Option<i32>,
    accepted_as_is: bool,
    deleted_entire_output: bool,
    re_recorded_quickly: bool,
    learning_candidate: bool,
    learning_modal_shown: bool,
    learning_confirmed: bool,
    learning_dismissed: bool,
    server_learning_saved: bool,
    server_learning_blocked: bool,
    has_numbers: bool,
    has_currency: bool,
    has_percent: bool,
    has_email: bool,
    has_url: bool,
    has_code_like_terms: bool,
    mixed_language: bool,
    protected_term_hit: bool,
    event_at_ms: i64,
}

#[derive(serde::Serialize)]
struct DailyRollupPayload {
    event_date: String,
    mode: String,
    run_count: i32,
    audio_seconds: f64,
    accepted_count: i32,
    edit_count: i32,
    heavy_edit_count: i32,
    learning_modal_shown: i32,
    learning_confirmed: i32,
    failure_count: i32,
    fallback_count: i32,
}

#[derive(serde::Deserialize)]
struct TelemetryBatchResponse {
    #[serde(default)]
    accepted_run_ids: Vec<String>,
    #[serde(default)]
    rejected_run_ids: Vec<String>,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn row_to_payload(row: &RunSummaryRow) -> RunSummaryPayload {
    RunSummaryPayload {
        run_id: row.run_id.clone(),
        recording_id: row.recording_id.clone(),
        device_id: row.device_id.clone(),
        mode: row.mode.clone(),
        target_app: row.target_app.clone(),
        platform: row.platform.clone(),
        app_version: row.app_version.clone(),
        machine_class: row.machine_class.clone(),
        audio_seconds: row.audio_seconds,
        word_count: row.word_count,
        char_count: row.char_count,
        transcribe_ms: row.transcribe_ms,
        embed_ms: row.embed_ms,
        polish_ms: row.polish_ms,
        total_ms: row.total_ms,
        paste_ms: row.paste_ms,
        success: row.success,
        error_code: row.error_code.clone(),
        used_clipboard_fallback: row.used_clipboard_fallback,
        used_ws_pretranscript: row.used_ws_pretranscript,
        used_http_stt_fallback: row.used_http_stt_fallback,
        stt_provider: row.stt_provider.clone(),
        stt_model: row.stt_model.clone(),
        stt_path: row.stt_path.clone(),
        edit_detected: row.edit_detected,
        edit_bucket: row.edit_bucket.clone(),
        edit_distance_chars: row.edit_distance_chars,
        edit_distance_words: row.edit_distance_words,
        accepted_as_is: row.accepted_as_is,
        deleted_entire_output: row.deleted_entire_output,
        re_recorded_quickly: row.re_recorded_quickly,
        learning_candidate: row.learning_candidate,
        learning_modal_shown: row.learning_modal_shown,
        learning_confirmed: row.learning_confirmed,
        learning_dismissed: row.learning_dismissed,
        server_learning_saved: row.server_learning_saved,
        server_learning_blocked: row.server_learning_blocked,
        has_numbers: row.has_numbers,
        has_currency: row.has_currency,
        has_percent: row.has_percent,
        has_email: row.has_email,
        has_url: row.has_url,
        has_code_like_terms: row.has_code_like_terms,
        mixed_language: row.mixed_language,
        protected_term_hit: row.protected_term_hit,
        event_at_ms: row.updated_at_ms,
    }
}

fn rollup_to_payload(row: &DailyRollupRow) -> DailyRollupPayload {
    DailyRollupPayload {
        event_date: row.event_date.clone(),
        mode: row.mode.clone(),
        run_count: row.run_count,
        audio_seconds: row.audio_seconds,
        accepted_count: row.accepted_count,
        edit_count: row.edit_count,
        heavy_edit_count: row.heavy_edit_count,
        learning_modal_shown: row.learning_modal_shown,
        learning_confirmed: row.learning_confirmed,
        failure_count: row.failure_count,
        fallback_count: row.fallback_count,
    }
}

/// Upload pending telemetry rows when the user is signed in. Best-effort only.
pub async fn upload_pending(
    pool: &DbPool,
    user_id: &str,
    http: &reqwest::Client,
    client_version: &str,
    device_id: &str,
) {
    let _ = telemetry::finalize_stale_runs(pool, user_id, 45_000);

    let user = match users::get_user(pool, user_id) {
        Some(u) => u,
        None => return,
    };
    let Some(token) = user
        .cloud_token
        .as_deref()
        .filter(|t| !t.trim().is_empty())
        .map(str::to_string)
    else {
        debug!("[telemetry] no cloud token — skipping upload");
        return;
    };

    let runs = match telemetry::list_ready_runs(pool, user_id, BATCH_RUN_LIMIT) {
        Ok(r) => r,
        Err(e) => {
            warn!("[telemetry] list_ready_runs failed: {e}");
            return;
        }
    };
    if runs.is_empty() {
        return;
    }

    let rollups =
        telemetry::list_ready_rollups(pool, user_id, BATCH_ROLLUP_LIMIT).unwrap_or_default();

    let base_url = user
        .enterprise_server_url
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| "https://airnote.emiactech.com".to_string());
    let url = format!(
        "{}/v1/runtime/telemetry/batch",
        base_url.trim_end_matches('/')
    );

    let payload = TelemetryBatchRequest {
        run_summaries: runs.iter().map(row_to_payload).collect(),
        daily_rollups: rollups.iter().map(rollup_to_payload).collect(),
        client_version: client_version.to_string(),
        device_id: device_id.to_string(),
        sent_at: now_ms(),
    };

    let req = cp_client::with_org_context(
        http.post(&url).bearer_auth(&token).json(&payload),
        Some(&user),
    );

    match req.timeout(std::time::Duration::from_secs(20)).send().await {
        Ok(resp) if resp.status().is_success() => {
            let body: TelemetryBatchResponse = resp.json().await.unwrap_or_default();
            let acked = if body.accepted_run_ids.is_empty() {
                runs.iter().map(|r| r.run_id.clone()).collect::<Vec<_>>()
            } else {
                body.accepted_run_ids
            };
            if let Err(e) = telemetry::mark_runs_uploaded(pool, user_id, &acked) {
                warn!("[telemetry] mark_runs_uploaded failed: {e}");
            } else {
                info!("[telemetry] uploaded {} run(s)", acked.len());
            }
        }
        Ok(resp) => {
            warn!("[telemetry] upload rejected: {}", resp.status());
        }
        Err(e) => {
            debug!("[telemetry] upload failed: {e}");
        }
    }
}

impl Default for TelemetryBatchResponse {
    fn default() -> Self {
        Self {
            accepted_run_ids: vec![],
            rejected_run_ids: vec![],
        }
    }
}

pub fn spawn_uploader(
    pool: crate::store::DbPool,
    user_id: String,
    http: reqwest::Client,
    client_version: String,
    device_id: String,
) {
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(15)).await;
        upload_pending(&pool, &user_id, &http, &client_version, &device_id).await;

        let mut interval = tokio::time::interval(std::time::Duration::from_secs(5 * 60));
        loop {
            interval.tick().await;
            if telemetry::should_upload(&pool, &user_id) {
                upload_pending(&pool, &user_id, &http, &client_version, &device_id).await;
            }
        }
    });
}

pub fn maybe_upload_after_run(pool: &DbPool, user_id: &str, http: &reqwest::Client) {
    if !telemetry::should_upload(pool, user_id) {
        return;
    }
    let pool = pool.clone();
    let user_id = user_id.to_string();
    let http = http.clone();
    let version = env!("CARGO_PKG_VERSION").to_string();
    let device_id = said_core::paths::device_id();
    tokio::spawn(async move {
        upload_pending(&pool, &user_id, &http, &version, &device_id).await;
    });
}
