//! Runtime telemetry batch ingest + org analytics.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{AppState, auth::AuthUser, tenant};

#[derive(Deserialize)]
pub struct TelemetryBatch {
    #[serde(default)]
    pub run_summaries: Vec<RunSummaryIn>,
    #[serde(default)]
    pub daily_rollups: Vec<DailyRollupIn>,
    pub client_version: Option<String>,
    pub device_id: Option<String>,
    pub sent_at: Option<i64>,
}

#[derive(Deserialize)]
pub struct RunSummaryIn {
    pub run_id: String,
    pub recording_id: Option<String>,
    pub device_id: Option<String>,
    pub mode: Option<String>,
    pub target_app: Option<String>,
    pub platform: Option<String>,
    pub app_version: Option<String>,
    pub machine_class: Option<String>,
    pub audio_seconds: Option<f64>,
    pub word_count: Option<i32>,
    pub char_count: Option<i32>,
    pub transcribe_ms: Option<i32>,
    pub embed_ms: Option<i32>,
    pub polish_ms: Option<i32>,
    pub total_ms: Option<i32>,
    pub paste_ms: Option<i32>,
    #[serde(default)]
    pub success: bool,
    pub error_code: Option<String>,
    #[serde(default)]
    pub used_clipboard_fallback: bool,
    pub speech_model: Option<String>,
    pub speech_path: Option<String>,
    #[serde(default)]
    pub edit_detected: bool,
    pub edit_bucket: Option<String>,
    pub edit_distance_chars: Option<i32>,
    pub edit_distance_words: Option<i32>,
    #[serde(default)]
    pub accepted_as_is: bool,
    #[serde(default)]
    pub deleted_entire_output: bool,
    #[serde(default)]
    pub re_recorded_quickly: bool,
    #[serde(default)]
    pub learning_candidate: bool,
    #[serde(default)]
    pub learning_modal_shown: bool,
    #[serde(default)]
    pub learning_confirmed: bool,
    #[serde(default)]
    pub learning_dismissed: bool,
    #[serde(default)]
    pub server_learning_saved: bool,
    #[serde(default)]
    pub server_learning_blocked: bool,
    #[serde(default)]
    pub has_numbers: bool,
    #[serde(default)]
    pub has_currency: bool,
    #[serde(default)]
    pub has_percent: bool,
    #[serde(default)]
    pub has_email: bool,
    #[serde(default)]
    pub has_url: bool,
    #[serde(default)]
    pub has_code_like_terms: bool,
    #[serde(default)]
    pub mixed_language: bool,
    #[serde(default)]
    pub protected_term_hit: bool,
    pub event_at_ms: Option<i64>,
}

#[derive(Deserialize)]
pub struct DailyRollupIn {
    pub event_date: String,
    pub mode: Option<String>,
    #[serde(default)]
    pub run_count: i32,
    #[serde(default)]
    pub audio_seconds: f64,
    #[serde(default)]
    pub accepted_count: i32,
    #[serde(default)]
    pub edit_count: i32,
    #[serde(default)]
    pub heavy_edit_count: i32,
    #[serde(default)]
    pub learning_modal_shown: i32,
    #[serde(default)]
    pub learning_confirmed: i32,
    #[serde(default)]
    pub failure_count: i32,
    #[serde(default)]
    pub fallback_count: i32,
}

#[derive(Serialize)]
pub struct TelemetryBatchResponse {
    pub accepted_run_ids: Vec<String>,
    pub rejected_run_ids: Vec<String>,
    pub accepted_count: usize,
    pub rejected_count: usize,
}

pub async fn batch_ingest(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Json(body): Json<TelemetryBatch>,
) -> Result<Json<TelemetryBatchResponse>, StatusCode> {
    let tenant = tenant::resolve_tenant(&state, &user, &headers)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let org_id = tenant.active_org_id;

    let mut accepted = Vec::new();
    let mut rejected = Vec::new();

    for run in &body.run_summaries {
        if run.run_id.trim().is_empty() {
            continue;
        }
        let event_at = run
            .event_at_ms
            .and_then(ms_to_datetime)
            .unwrap_or_else(Utc::now);
        let mode = run.mode.as_deref().unwrap_or("normal_voice");
        let edit_bucket = run.edit_bucket.as_deref().unwrap_or("none");

        let result = sqlx::query(
            "INSERT INTO runtime_telemetry_runs (
                account_id, org_id, run_id, recording_id, device_id, mode, target_app, platform,
                app_version, machine_class, audio_seconds, word_count, char_count, transcribe_ms,
                embed_ms, polish_ms, total_ms, paste_ms, success, error_code,
                used_clipboard_fallback, speech_model, speech_path, edit_detected, edit_bucket,
                edit_distance_chars, edit_distance_words,
                accepted_as_is, deleted_entire_output, re_recorded_quickly, learning_candidate,
                learning_modal_shown, learning_confirmed, learning_dismissed, server_learning_saved,
                server_learning_blocked, has_numbers, has_currency, has_percent, has_email, has_url,
                has_code_like_terms, mixed_language, protected_term_hit, client_version, event_at
            ) VALUES (
                $1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22,$23,
                $24,$25,$26,$27,$28,$29,$30,$31,$32,$33,$34,$35,$36,$37,$38,$39,$40,$41,$42,$43,$44,$45,$46
            )
            ON CONFLICT (account_id, run_id) DO NOTHING",
        )
        .bind(user.account_id)
        .bind(org_id)
        .bind(&run.run_id)
        .bind(&run.recording_id)
        .bind(run.device_id.as_deref().or(body.device_id.as_deref()))
        .bind(mode)
        .bind(&run.target_app)
        .bind(&run.platform)
        .bind(run.app_version.as_deref().or(body.client_version.as_deref()))
        .bind(&run.machine_class)
        .bind(run.audio_seconds)
        .bind(run.word_count)
        .bind(run.char_count)
        .bind(run.transcribe_ms)
        .bind(run.embed_ms)
        .bind(run.polish_ms)
        .bind(run.total_ms)
        .bind(run.paste_ms)
        .bind(run.success)
        .bind(&run.error_code)
        .bind(run.used_clipboard_fallback)
        .bind(&run.speech_model)
        .bind(&run.speech_path)
        .bind(run.edit_detected)
        .bind(edit_bucket)
        .bind(run.edit_distance_chars)
        .bind(run.edit_distance_words)
        .bind(run.accepted_as_is)
        .bind(run.deleted_entire_output)
        .bind(run.re_recorded_quickly)
        .bind(run.learning_candidate)
        .bind(run.learning_modal_shown)
        .bind(run.learning_confirmed)
        .bind(run.learning_dismissed)
        .bind(run.server_learning_saved)
        .bind(run.server_learning_blocked)
        .bind(run.has_numbers)
        .bind(run.has_currency)
        .bind(run.has_percent)
        .bind(run.has_email)
        .bind(run.has_url)
        .bind(run.has_code_like_terms)
        .bind(run.mixed_language)
        .bind(run.protected_term_hit)
        .bind(body.client_version.as_deref())
        .bind(event_at)
        .execute(&state.db)
        .await;

        match result {
            Ok(r) if r.rows_affected() > 0 => accepted.push(run.run_id.clone()),
            Ok(_) => accepted.push(run.run_id.clone()),
            Err(_) => rejected.push(run.run_id.clone()),
        }
    }

    for rollup in &body.daily_rollups {
        let Ok(date) = NaiveDate::parse_from_str(&rollup.event_date, "%Y-%m-%d") else {
            continue;
        };
        let mode = rollup.mode.as_deref().unwrap_or("all");
        let _ = sqlx::query(
            "INSERT INTO runtime_telemetry_daily (
                org_id, account_id, event_date, mode, run_count, audio_seconds, accepted_count,
                edit_count, heavy_edit_count, learning_modal_shown, learning_confirmed,
                failure_count, fallback_count, updated_at
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,now())
            ON CONFLICT (account_id, event_date, mode) DO UPDATE SET
                run_count = runtime_telemetry_daily.run_count + EXCLUDED.run_count,
                audio_seconds = runtime_telemetry_daily.audio_seconds + EXCLUDED.audio_seconds,
                accepted_count = runtime_telemetry_daily.accepted_count + EXCLUDED.accepted_count,
                edit_count = runtime_telemetry_daily.edit_count + EXCLUDED.edit_count,
                heavy_edit_count = runtime_telemetry_daily.heavy_edit_count + EXCLUDED.heavy_edit_count,
                learning_modal_shown = runtime_telemetry_daily.learning_modal_shown + EXCLUDED.learning_modal_shown,
                learning_confirmed = runtime_telemetry_daily.learning_confirmed + EXCLUDED.learning_confirmed,
                failure_count = runtime_telemetry_daily.failure_count + EXCLUDED.failure_count,
                fallback_count = runtime_telemetry_daily.fallback_count + EXCLUDED.fallback_count,
                updated_at = now()",
        )
        .bind(org_id)
        .bind(user.account_id)
        .bind(date)
        .bind(mode)
        .bind(rollup.run_count)
        .bind(rollup.audio_seconds)
        .bind(rollup.accepted_count)
        .bind(rollup.edit_count)
        .bind(rollup.heavy_edit_count)
        .bind(rollup.learning_modal_shown)
        .bind(rollup.learning_confirmed)
        .bind(rollup.failure_count)
        .bind(rollup.fallback_count)
        .execute(&state.db)
        .await;
    }

    let _ = sqlx::query(
        "INSERT INTO runtime_telemetry_uploads
            (account_id, org_id, device_id, client_version, run_count, rollup_count, accepted_count, rejected_count)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
    )
    .bind(user.account_id)
    .bind(org_id)
    .bind(body.device_id.as_deref())
    .bind(body.client_version.as_deref())
    .bind(accepted.len() as i32)
    .bind(body.daily_rollups.len() as i32)
    .bind(accepted.len() as i32)
    .bind(rejected.len() as i32)
    .execute(&state.db)
    .await;

    Ok(Json(TelemetryBatchResponse {
        accepted_count: accepted.len(),
        rejected_count: rejected.len(),
        accepted_run_ids: accepted,
        rejected_run_ids: rejected,
    }))
}

#[derive(Deserialize)]
pub struct AnalyticsQuery {
    pub days: Option<i32>,
}

#[derive(Deserialize)]
pub struct UsersListQuery {
    pub days: Option<i32>,
    pub q: Option<String>,
    pub limit: Option<i32>,
    pub offset: Option<i32>,
}

#[derive(Deserialize)]
pub struct UserRunsQuery {
    pub days: Option<i32>,
    pub mode: Option<String>,
    pub limit: Option<i32>,
    pub offset: Option<i32>,
}

#[derive(Debug, Clone, Copy)]
struct RunTotals {
    run_count: i64,
    audio_seconds: f64,
    word_count: i64,
    char_count: i64,
    accepted: i64,
    edits: i64,
    heavy_edits: i64,
    fallbacks: i64,
    learning_candidates: i64,
    learning_saved: i64,
    learning_modal_shown: i64,
    learning_confirmed: i64,
    learning_dismissed: i64,
    server_learning_blocked: i64,
    deleted_entire_output: i64,
    re_recorded_quickly: i64,
    failures: i64,
}

#[derive(Debug, Clone, Copy)]
struct LatencyPercentiles {
    total_p50: Option<f64>,
    total_p95: Option<f64>,
    transcribe_p50: Option<f64>,
    transcribe_p95: Option<f64>,
    embed_p50: Option<f64>,
    embed_p95: Option<f64>,
    polish_p50: Option<f64>,
    polish_p95: Option<f64>,
    paste_p50: Option<f64>,
    paste_p95: Option<f64>,
}

fn telemetry_rate(n: i64, total: i64) -> f64 {
    if total == 0 {
        0.0
    } else {
        n as f64 / total as f64
    }
}

fn window_days(days: Option<i32>) -> (i32, DateTime<Utc>) {
    let days = days.unwrap_or(30).clamp(1, 90);
    let since = Utc::now() - chrono::Duration::days(days as i64);
    (days, since)
}

fn require_org_viewer(role: &str) -> Result<(), StatusCode> {
    if role.eq_ignore_ascii_case("admin")
        || role.eq_ignore_ascii_case("owner")
        || role.eq_ignore_ascii_case("viewer")
        || role.eq_ignore_ascii_case("COMPANY_ADMIN")
        || role.eq_ignore_ascii_case("MANAGER")
        || role.eq_ignore_ascii_case("member")
        || role.eq_ignore_ascii_case("MEMBER")
    {
        Ok(())
    } else {
        Err(StatusCode::FORBIDDEN)
    }
}

pub async fn org_analytics(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Path(org_id): Path<Uuid>,
    Query(q): Query<AnalyticsQuery>,
) -> Result<Json<Value>, StatusCode> {
    let role = tenant::require_org_membership(&state, user.account_id, org_id)
        .await
        .map_err(|_| StatusCode::FORBIDDEN)?;
    require_org_viewer(&role)?;
    let tenant = tenant::resolve_tenant(&state, &user, &headers)
        .await
        .map_err(|_| StatusCode::FORBIDDEN)?;
    if tenant::multi_org_enabled()
        && tenant.active_org_id.is_some()
        && tenant.active_org_id != Some(org_id)
    {
        let is_admin =
            role.eq_ignore_ascii_case("admin") || role.eq_ignore_ascii_case("COMPANY_ADMIN");
        if !is_admin {
            return Err(StatusCode::FORBIDDEN);
        }
    }

    let (days, since) = window_days(q.days);
    let totals = fetch_run_totals(&state.db, org_id, None, since)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let run_count = totals.run_count;
    let audio_seconds = totals.audio_seconds;

    let latency = fetch_latency_percentiles(&state.db, org_id, None, since)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let dau: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT account_id)::bigint
           FROM runtime_telemetry_runs
          WHERE org_id = $1 AND event_at >= now() - INTERVAL '1 day'",
    )
    .bind(org_id)
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);

    let wau: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT account_id)::bigint
           FROM runtime_telemetry_runs
          WHERE org_id = $1 AND event_at >= now() - INTERVAL '7 days'",
    )
    .bind(org_id)
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);

    let by_mode: Vec<(String, i64)> = sqlx::query_as(
        "SELECT mode, COUNT(*)::bigint
           FROM runtime_telemetry_runs
          WHERE org_id = $1 AND event_at >= $2
          GROUP BY mode
          ORDER BY COUNT(*) DESC",
    )
    .bind(org_id)
    .bind(since)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let by_app: Vec<(Option<String>, i64)> = sqlx::query_as(
        "SELECT target_app, COUNT(*)::bigint
           FROM runtime_telemetry_runs
          WHERE org_id = $1 AND event_at >= $2
          GROUP BY target_app
          ORDER BY COUNT(*) DESC
          LIMIT 10",
    )
    .bind(org_id)
    .bind(since)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let speech_rows = fetch_speech_breakdown(&state.db, org_id, None, since)
        .await
        .unwrap_or_default();

    Ok(Json(json!({
        "window_days": days,
        "usage": {
            "dau": dau,
            "wau": wau,
            "completed_runs": run_count,
            "audio_minutes": (audio_seconds / 60.0 * 10.0).round() / 10.0,
            "by_mode": by_mode.iter().map(|(m, c)| json!({"mode": m, "count": c})).collect::<Vec<_>>(),
            "by_target_app": by_app.iter().map(|(a, c)| json!({"target_app": a, "count": c})).collect::<Vec<_>>(),
        },
        "quality": quality_json(&totals),
        "latency_ms": latency_json(&latency),
        "speech": speech_breakdown_json(&speech_rows),
    })))
}

#[derive(sqlx::FromRow)]
struct RunTotalsRow {
    run_count: i64,
    audio_seconds: f64,
    word_count: i64,
    char_count: i64,
    accepted: i64,
    edits: i64,
    heavy_edits: i64,
    fallbacks: i64,
    learning_candidates: i64,
    learning_saved: i64,
    learning_modal_shown: i64,
    learning_confirmed: i64,
    learning_dismissed: i64,
    server_learning_blocked: i64,
    deleted_entire_output: i64,
    re_recorded_quickly: i64,
    failures: i64,
}

async fn fetch_run_totals(
    db: &sqlx::PgPool,
    org_id: Uuid,
    account_id: Option<Uuid>,
    since: DateTime<Utc>,
) -> Result<RunTotals, sqlx::Error> {
    let row: RunTotalsRow = sqlx::query_as(
        "SELECT
            COUNT(*)::bigint AS run_count,
            COALESCE(SUM(audio_seconds), 0)::float8 AS audio_seconds,
            COALESCE(SUM(word_count), 0)::bigint AS word_count,
            COALESCE(SUM(char_count), 0)::bigint AS char_count,
            COALESCE(SUM(CASE
                WHEN accepted_as_is THEN 1
                WHEN edit_detected THEN 0
                WHEN success AND edit_bucket = 'none' AND NOT deleted_entire_output THEN 1
                ELSE 0
            END), 0)::bigint AS accepted,
            COALESCE(SUM(CASE WHEN edit_detected THEN 1 ELSE 0 END), 0)::bigint AS edits,
            COALESCE(SUM(CASE WHEN edit_bucket IN ('medium','heavy','full_replace') THEN 1 ELSE 0 END), 0)::bigint AS heavy_edits,
            COALESCE(SUM(CASE WHEN used_clipboard_fallback THEN 1 ELSE 0 END), 0)::bigint AS fallbacks,
            COALESCE(SUM(CASE WHEN learning_candidate THEN 1 ELSE 0 END), 0)::bigint AS learning_candidates,
            COALESCE(SUM(CASE WHEN server_learning_saved THEN 1 ELSE 0 END), 0)::bigint AS learning_saved,
            COALESCE(SUM(CASE WHEN learning_modal_shown THEN 1 ELSE 0 END), 0)::bigint AS learning_modal_shown,
            COALESCE(SUM(CASE WHEN learning_confirmed THEN 1 ELSE 0 END), 0)::bigint AS learning_confirmed,
            COALESCE(SUM(CASE WHEN learning_dismissed THEN 1 ELSE 0 END), 0)::bigint AS learning_dismissed,
            COALESCE(SUM(CASE WHEN server_learning_blocked THEN 1 ELSE 0 END), 0)::bigint AS server_learning_blocked,
            COALESCE(SUM(CASE WHEN deleted_entire_output THEN 1 ELSE 0 END), 0)::bigint AS deleted_entire_output,
            COALESCE(SUM(CASE WHEN re_recorded_quickly THEN 1 ELSE 0 END), 0)::bigint AS re_recorded_quickly,
            COALESCE(SUM(CASE WHEN success = false THEN 1 ELSE 0 END), 0)::bigint AS failures
         FROM runtime_telemetry_runs
         WHERE org_id = $1 AND event_at >= $2
           AND ($3::uuid IS NULL OR account_id = $3)",
    )
    .bind(org_id)
    .bind(since)
    .bind(account_id)
    .fetch_one(db)
    .await?;

    Ok(RunTotals {
        run_count: row.run_count,
        audio_seconds: row.audio_seconds,
        word_count: row.word_count,
        char_count: row.char_count,
        accepted: row.accepted,
        edits: row.edits,
        heavy_edits: row.heavy_edits,
        fallbacks: row.fallbacks,
        learning_candidates: row.learning_candidates,
        learning_saved: row.learning_saved,
        learning_modal_shown: row.learning_modal_shown,
        learning_confirmed: row.learning_confirmed,
        learning_dismissed: row.learning_dismissed,
        server_learning_blocked: row.server_learning_blocked,
        deleted_entire_output: row.deleted_entire_output,
        re_recorded_quickly: row.re_recorded_quickly,
        failures: row.failures,
    })
}

async fn fetch_latency_percentiles(
    db: &sqlx::PgPool,
    org_id: Uuid,
    account_id: Option<Uuid>,
    since: DateTime<Utc>,
) -> Result<LatencyPercentiles, sqlx::Error> {
    let row: (
        Option<f64>,
        Option<f64>,
        Option<f64>,
        Option<f64>,
        Option<f64>,
        Option<f64>,
        Option<f64>,
        Option<f64>,
        Option<f64>,
        Option<f64>,
    ) = sqlx::query_as(
        "SELECT
            percentile_cont(0.5) WITHIN GROUP (ORDER BY total_ms),
            percentile_cont(0.95) WITHIN GROUP (ORDER BY total_ms),
            percentile_cont(0.5) WITHIN GROUP (ORDER BY transcribe_ms),
            percentile_cont(0.95) WITHIN GROUP (ORDER BY transcribe_ms),
            percentile_cont(0.5) WITHIN GROUP (ORDER BY embed_ms),
            percentile_cont(0.95) WITHIN GROUP (ORDER BY embed_ms),
            percentile_cont(0.5) WITHIN GROUP (ORDER BY polish_ms),
            percentile_cont(0.95) WITHIN GROUP (ORDER BY polish_ms),
            percentile_cont(0.5) WITHIN GROUP (ORDER BY paste_ms),
            percentile_cont(0.95) WITHIN GROUP (ORDER BY paste_ms)
         FROM runtime_telemetry_runs
         WHERE org_id = $1 AND event_at >= $2 AND success = true
           AND ($3::uuid IS NULL OR account_id = $3)",
    )
    .bind(org_id)
    .bind(since)
    .bind(account_id)
    .fetch_one(db)
    .await?;

    Ok(LatencyPercentiles {
        total_p50: row.0,
        total_p95: row.1,
        transcribe_p50: row.2,
        transcribe_p95: row.3,
        embed_p50: row.4,
        embed_p95: row.5,
        polish_p50: row.6,
        polish_p95: row.7,
        paste_p50: row.8,
        paste_p95: row.9,
    })
}

fn quality_json(totals: &RunTotals) -> Value {
    json!({
        "acceptance_rate": telemetry_rate(totals.accepted, totals.run_count),
        "edit_rate": telemetry_rate(totals.edits, totals.run_count),
        "heavy_edit_rate": telemetry_rate(totals.heavy_edits, totals.run_count),
        "fallback_rate": telemetry_rate(totals.fallbacks, totals.run_count),
        "learning_candidate_rate": telemetry_rate(totals.learning_candidates, totals.run_count),
        "learning_success_rate": telemetry_rate(totals.learning_saved, totals.learning_candidates),
    })
}

fn latency_json(latency: &LatencyPercentiles) -> Value {
    json!({
        "total_p50": latency.total_p50,
        "total_p95": latency.total_p95,
        "transcribe_p50": latency.transcribe_p50,
        "transcribe_p95": latency.transcribe_p95,
        "embed_p50": latency.embed_p50,
        "embed_p95": latency.embed_p95,
        "polish_p50": latency.polish_p50,
        "polish_p95": latency.polish_p95,
        "paste_p50": latency.paste_p50,
        "paste_p95": latency.paste_p95,
    })
}

fn speech_short_label(model: &str) -> &str {
    model
}

fn primary_speech_label(model: &str, share_pct: f64) -> String {
    format!("{} {:.0}%", speech_short_label(model), share_pct)
}

async fn fetch_speech_breakdown(
    db: &sqlx::PgPool,
    org_id: Uuid,
    account_id: Option<Uuid>,
    since: DateTime<Utc>,
) -> Result<Vec<(String, String, i64)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT COALESCE(speech_model, 'unknown') AS speech_model,
                COALESCE(speech_path, 'unknown') AS speech_path,
                COUNT(*)::bigint
           FROM runtime_telemetry_runs
          WHERE org_id = $1 AND event_at >= $2
            AND ($3::uuid IS NULL OR account_id = $3)
            AND speech_model IS NOT NULL
          GROUP BY speech_model, speech_path
          ORDER BY COUNT(*) DESC",
    )
    .bind(org_id)
    .bind(since)
    .bind(account_id)
    .fetch_all(db)
    .await
}

fn speech_breakdown_json(rows: &[(String, String, i64)]) -> Value {
    let by_model_path: Vec<Value> = rows
        .iter()
        .map(|(model, path, count)| {
            json!({
                "speech_model": model,
                "speech_path": path,
                "count": count,
            })
        })
        .collect();
    let mut model_totals: std::collections::BTreeMap<String, i64> =
        std::collections::BTreeMap::new();
    for (model, _, count) in rows {
        *model_totals.entry(model.clone()).or_default() += count;
    }
    let total: i64 = model_totals.values().sum();
    let by_model: Vec<Value> = model_totals
        .into_iter()
        .map(|(model, count)| {
            json!({
                "speech_model": model,
                "count": count,
                "share": if total > 0 {
                    (count as f64 * 100.0 / total as f64 * 10.0).round() / 10.0
                } else {
                    0.0
                },
            })
        })
        .collect();
    json!({
        "by_model_path": by_model_path,
        "by_model": by_model,
        "total_tagged": total,
    })
}

async fn fetch_speech_latency_by_model(
    db: &sqlx::PgPool,
    org_id: Uuid,
    account_id: Uuid,
    since: DateTime<Utc>,
) -> Result<Vec<Value>, sqlx::Error> {
    let rows: Vec<(String, Option<f64>, Option<f64>, i64)> = sqlx::query_as(
        "SELECT COALESCE(speech_model, 'unknown') AS speech_model,
                percentile_cont(0.5) WITHIN GROUP (ORDER BY transcribe_ms),
                percentile_cont(0.95) WITHIN GROUP (ORDER BY transcribe_ms),
                COUNT(*)::bigint
           FROM runtime_telemetry_runs
          WHERE org_id = $1 AND account_id = $2 AND event_at >= $3
            AND success = true AND speech_model IS NOT NULL
          GROUP BY speech_model
          ORDER BY COUNT(*) DESC",
    )
    .bind(org_id)
    .bind(account_id)
    .bind(since)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(model, p50, p95, runs)| {
            json!({
                "speech_model": model,
                "transcribe_p50": p50,
                "transcribe_p95": p95,
                "runs": runs,
            })
        })
        .collect())
}

async fn ensure_org_account_member(
    db: &sqlx::PgPool,
    org_id: Uuid,
    account_id: Uuid,
) -> Result<(), StatusCode> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM org_members WHERE org_id = $1 AND account_id = $2
        )",
    )
    .bind(org_id)
    .bind(account_id)
    .fetch_one(db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if exists {
        Ok(())
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

pub async fn list_users(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Path(org_id): Path<Uuid>,
    Query(q): Query<UsersListQuery>,
) -> Result<Json<Value>, StatusCode> {
    let (_, role) = tenant::ensure_path_org_active(&state, &user, &headers, org_id)
        .await
        .map_err(|_| StatusCode::FORBIDDEN)?;
    require_org_viewer(&role)?;

    let (days, since) = window_days(q.days);
    let limit = q.limit.unwrap_or(50).clamp(1, 200) as i64;
    let offset = q.offset.unwrap_or(0).max(0) as i64;
    let search =
        q.q.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| format!("%{s}%"));

    let total: i64 = if let Some(ref pattern) = search {
        sqlx::query_scalar(
            "SELECT COUNT(*)::bigint
               FROM org_members om
               JOIN accounts a ON a.id = om.account_id
              WHERE om.org_id = $1
                AND (a.email ILIKE $2 OR om.lark_name ILIKE $2)",
        )
        .bind(org_id)
        .bind(pattern)
        .fetch_one(&state.db)
        .await
        .unwrap_or(0)
    } else {
        sqlx::query_scalar("SELECT COUNT(*)::bigint FROM org_members WHERE org_id = $1")
            .bind(org_id)
            .fetch_one(&state.db)
            .await
            .unwrap_or(0)
    };

    #[derive(sqlx::FromRow)]
    struct TelemetryUserListRow {
        account_id: Uuid,
        email: String,
        lark_name: Option<String>,
        role: String,
        auth_source: String,
        runs: i64,
        audio_seconds: f64,
        accepted: i64,
        edits: i64,
        heavy_edits: i64,
        fallbacks: i64,
        learning_candidates: i64,
        learning_saved: i64,
        last_active_at: Option<DateTime<Utc>>,
        desktop_active: bool,
        primary_speech_model: Option<String>,
        primary_speech_count: Option<i64>,
    }

    let rows: Vec<TelemetryUserListRow> = if let Some(ref pattern) = search {
        sqlx::query_as(
            "SELECT
                om.account_id,
                a.email,
                om.lark_name,
                om.role,
                CASE
                  WHEN om.lark_user_id IS NOT NULL THEN 'lark'
                  WHEN om.auth_source IS NOT NULL THEN om.auth_source
                  ELSE 'email'
                END AS auth_source,
                COALESCE(agg.runs, 0)::bigint AS runs,
                COALESCE(agg.audio_seconds, 0)::float8 AS audio_seconds,
                COALESCE(agg.accepted, 0)::bigint AS accepted,
                COALESCE(agg.edits, 0)::bigint AS edits,
                COALESCE(agg.heavy_edits, 0)::bigint AS heavy_edits,
                COALESCE(agg.fallbacks, 0)::bigint AS fallbacks,
                COALESCE(agg.learning_candidates, 0)::bigint AS learning_candidates,
                COALESCE(agg.learning_saved, 0)::bigint AS learning_saved,
                agg.last_active_at,
                COALESCE(dc.desktop_active, false) AS desktop_active,
                speech_top.speech_model AS primary_speech_model,
                speech_top.cnt AS primary_speech_count
             FROM org_members om
             JOIN accounts a ON a.id = om.account_id
             LEFT JOIN (
                SELECT account_id,
                       COUNT(*)::bigint AS runs,
                       COALESCE(SUM(audio_seconds), 0)::float8 AS audio_seconds,
                       COALESCE(SUM(CASE
                WHEN accepted_as_is THEN 1
                WHEN edit_detected THEN 0
                WHEN success AND edit_bucket = 'none' AND NOT deleted_entire_output THEN 1
                ELSE 0
            END), 0)::bigint AS accepted,
                       COALESCE(SUM(CASE WHEN edit_detected THEN 1 ELSE 0 END), 0)::bigint AS edits,
                       COALESCE(SUM(CASE WHEN edit_bucket IN ('medium','heavy','full_replace') THEN 1 ELSE 0 END), 0)::bigint AS heavy_edits,
                       COALESCE(SUM(CASE WHEN used_clipboard_fallback THEN 1 ELSE 0 END), 0)::bigint AS fallbacks,
                       COALESCE(SUM(CASE WHEN learning_candidate THEN 1 ELSE 0 END), 0)::bigint AS learning_candidates,
                       COALESCE(SUM(CASE WHEN server_learning_saved THEN 1 ELSE 0 END), 0)::bigint AS learning_saved,
                       MAX(event_at) AS last_active_at
                  FROM runtime_telemetry_runs
                 WHERE org_id = $1 AND event_at >= $2
                 GROUP BY account_id
             ) agg ON agg.account_id = om.account_id
             LEFT JOIN LATERAL (
                SELECT bool_or(last_seen_at > now() - INTERVAL '15 minutes') AS desktop_active
                  FROM desktop_clients dc
                 WHERE dc.org_id = $1 AND dc.account_id = om.account_id
             ) dc ON true
             LEFT JOIN LATERAL (
                SELECT r.speech_model, COUNT(*)::bigint AS cnt
                  FROM runtime_telemetry_runs r
                 WHERE r.org_id = $1 AND r.account_id = om.account_id
                   AND r.event_at >= $2 AND r.speech_model IS NOT NULL
                 GROUP BY r.speech_model
                 ORDER BY cnt DESC
                 LIMIT 1
             ) speech_top ON true
             WHERE om.org_id = $1
               AND (a.email ILIKE $3 OR om.lark_name ILIKE $3)
             ORDER BY COALESCE(agg.runs, 0) DESC, om.joined_at ASC
             LIMIT $4 OFFSET $5",
        )
        .bind(org_id)
        .bind(since)
        .bind(pattern)
        .bind(limit)
        .bind(offset)
        .fetch_all(&state.db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    } else {
        sqlx::query_as(
            "SELECT
                om.account_id,
                a.email,
                om.lark_name,
                om.role,
                CASE
                  WHEN om.lark_user_id IS NOT NULL THEN 'lark'
                  WHEN om.auth_source IS NOT NULL THEN om.auth_source
                  ELSE 'email'
                END AS auth_source,
                COALESCE(agg.runs, 0)::bigint AS runs,
                COALESCE(agg.audio_seconds, 0)::float8 AS audio_seconds,
                COALESCE(agg.accepted, 0)::bigint AS accepted,
                COALESCE(agg.edits, 0)::bigint AS edits,
                COALESCE(agg.heavy_edits, 0)::bigint AS heavy_edits,
                COALESCE(agg.fallbacks, 0)::bigint AS fallbacks,
                COALESCE(agg.learning_candidates, 0)::bigint AS learning_candidates,
                COALESCE(agg.learning_saved, 0)::bigint AS learning_saved,
                agg.last_active_at,
                COALESCE(dc.desktop_active, false) AS desktop_active,
                speech_top.speech_model AS primary_speech_model,
                speech_top.cnt AS primary_speech_count
             FROM org_members om
             JOIN accounts a ON a.id = om.account_id
             LEFT JOIN (
                SELECT account_id,
                       COUNT(*)::bigint AS runs,
                       COALESCE(SUM(audio_seconds), 0)::float8 AS audio_seconds,
                       COALESCE(SUM(CASE
                WHEN accepted_as_is THEN 1
                WHEN edit_detected THEN 0
                WHEN success AND edit_bucket = 'none' AND NOT deleted_entire_output THEN 1
                ELSE 0
            END), 0)::bigint AS accepted,
                       COALESCE(SUM(CASE WHEN edit_detected THEN 1 ELSE 0 END), 0)::bigint AS edits,
                       COALESCE(SUM(CASE WHEN edit_bucket IN ('medium','heavy','full_replace') THEN 1 ELSE 0 END), 0)::bigint AS heavy_edits,
                       COALESCE(SUM(CASE WHEN used_clipboard_fallback THEN 1 ELSE 0 END), 0)::bigint AS fallbacks,
                       COALESCE(SUM(CASE WHEN learning_candidate THEN 1 ELSE 0 END), 0)::bigint AS learning_candidates,
                       COALESCE(SUM(CASE WHEN server_learning_saved THEN 1 ELSE 0 END), 0)::bigint AS learning_saved,
                       MAX(event_at) AS last_active_at
                  FROM runtime_telemetry_runs
                 WHERE org_id = $1 AND event_at >= $2
                 GROUP BY account_id
             ) agg ON agg.account_id = om.account_id
             LEFT JOIN LATERAL (
                SELECT bool_or(last_seen_at > now() - INTERVAL '15 minutes') AS desktop_active
                  FROM desktop_clients dc
                 WHERE dc.org_id = $1 AND dc.account_id = om.account_id
             ) dc ON true
             LEFT JOIN LATERAL (
                SELECT r.speech_model, COUNT(*)::bigint AS cnt
                  FROM runtime_telemetry_runs r
                 WHERE r.org_id = $1 AND r.account_id = om.account_id
                   AND r.event_at >= $2 AND r.speech_model IS NOT NULL
                 GROUP BY r.speech_model
                 ORDER BY cnt DESC
                 LIMIT 1
             ) speech_top ON true
             WHERE om.org_id = $1
             ORDER BY COALESCE(agg.runs, 0) DESC, om.joined_at ASC
             LIMIT $3 OFFSET $4",
        )
        .bind(org_id)
        .bind(since)
        .bind(limit)
        .bind(offset)
        .fetch_all(&state.db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };

    let users: Vec<Value> = rows
        .into_iter()
        .map(|row| {
            json!({
                "account_id": row.account_id,
                "email": row.email,
                "lark_name": row.lark_name,
                "role": row.role,
                "auth_source": row.auth_source,
                "runs": row.runs,
                "audio_minutes": (row.audio_seconds / 60.0 * 10.0).round() / 10.0,
                "acceptance_rate": telemetry_rate(row.accepted, row.runs),
                "edit_rate": telemetry_rate(row.edits, row.runs),
                "heavy_edit_rate": telemetry_rate(row.heavy_edits, row.runs),
                "fallback_rate": telemetry_rate(row.fallbacks, row.runs),
                "learning_success_rate": telemetry_rate(row.learning_saved, row.learning_candidates),
                "last_active_at": row.last_active_at,
                "desktop_active": row.desktop_active,
                "primary_speech": match (
                    row.primary_speech_model.as_deref(),
                    row.primary_speech_count,
                    row.runs,
                ) {
                    (Some(model), Some(cnt), runs) if runs > 0 => {
                        Some(primary_speech_label(model, cnt as f64 * 100.0 / runs as f64))
                    }
                    _ => None,
                },
            })
        })
        .collect();

    Ok(Json(json!({
        "window_days": days,
        "total": total,
        "limit": limit,
        "offset": offset,
        "users": users,
    })))
}

pub async fn user_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Path((org_id, account_id)): Path<(Uuid, Uuid)>,
    Query(q): Query<AnalyticsQuery>,
) -> Result<Json<Value>, StatusCode> {
    let (_, role) = tenant::ensure_path_org_active(&state, &user, &headers, org_id)
        .await
        .map_err(|_| StatusCode::FORBIDDEN)?;
    require_org_viewer(&role)?;
    ensure_org_account_member(&state.db, org_id, account_id).await?;

    let (days, since) = window_days(q.days);

    let member: Option<(String, Option<String>, Option<String>, String, String, bool)> =
        sqlx::query_as(
            "SELECT a.email, om.lark_name, om.lark_department, om.role,
                CASE
                  WHEN om.lark_user_id IS NOT NULL THEN 'lark'
                  WHEN om.auth_source IS NOT NULL THEN om.auth_source
                  ELSE 'email'
                END,
                (om.lark_user_id IS NOT NULL)
           FROM org_members om
           JOIN accounts a ON a.id = om.account_id
          WHERE om.org_id = $1 AND om.account_id = $2",
        )
        .bind(org_id)
        .bind(account_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let (email, lark_name, lark_department, member_role, auth_source, lark_connected) =
        member.ok_or(StatusCode::NOT_FOUND)?;

    let desktop_active: bool = sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM desktop_clients
             WHERE org_id = $1 AND account_id = $2
               AND last_seen_at > now() - INTERVAL '15 minutes'
        )",
    )
    .bind(org_id)
    .bind(account_id)
    .fetch_one(&state.db)
    .await
    .unwrap_or(false);

    let totals = fetch_run_totals(&state.db, org_id, Some(account_id), since)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let latency = fetch_latency_percentiles(&state.db, org_id, Some(account_id), since)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let by_mode: Vec<(String, i64)> = sqlx::query_as(
        "SELECT mode, COUNT(*)::bigint
           FROM runtime_telemetry_runs
          WHERE org_id = $1 AND account_id = $2 AND event_at >= $3
          GROUP BY mode ORDER BY COUNT(*) DESC",
    )
    .bind(org_id)
    .bind(account_id)
    .bind(since)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let by_app: Vec<(Option<String>, i64)> = sqlx::query_as(
        "SELECT target_app, COUNT(*)::bigint
           FROM runtime_telemetry_runs
          WHERE org_id = $1 AND account_id = $2 AND event_at >= $3
          GROUP BY target_app ORDER BY COUNT(*) DESC LIMIT 10",
    )
    .bind(org_id)
    .bind(account_id)
    .bind(since)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let speech_rows = fetch_speech_breakdown(&state.db, org_id, Some(account_id), since)
        .await
        .unwrap_or_default();
    let speech_latency = fetch_speech_latency_by_model(&state.db, org_id, account_id, since)
        .await
        .unwrap_or_default();

    let flags: (i64, i64, i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT
            COALESCE(SUM(CASE WHEN has_numbers THEN 1 ELSE 0 END), 0)::bigint,
            COALESCE(SUM(CASE WHEN has_currency THEN 1 ELSE 0 END), 0)::bigint,
            COALESCE(SUM(CASE WHEN has_percent THEN 1 ELSE 0 END), 0)::bigint,
            COALESCE(SUM(CASE WHEN has_email THEN 1 ELSE 0 END), 0)::bigint,
            COALESCE(SUM(CASE WHEN has_url THEN 1 ELSE 0 END), 0)::bigint,
            COALESCE(SUM(CASE WHEN has_code_like_terms THEN 1 ELSE 0 END), 0)::bigint,
            COALESCE(SUM(CASE WHEN mixed_language THEN 1 ELSE 0 END), 0)::bigint,
            COALESCE(SUM(CASE WHEN protected_term_hit THEN 1 ELSE 0 END), 0)::bigint
         FROM runtime_telemetry_runs
         WHERE org_id = $1 AND account_id = $2 AND event_at >= $3",
    )
    .bind(org_id)
    .bind(account_id)
    .bind(since)
    .fetch_one(&state.db)
    .await
    .unwrap_or((0, 0, 0, 0, 0, 0, 0, 0));

    let daily_rollups: Vec<(
        NaiveDate,
        String,
        i32,
        f64,
        i32,
        i32,
        i32,
        i32,
        i32,
        i32,
        i32,
    )> = sqlx::query_as(
        "SELECT event_date, mode, run_count, audio_seconds, accepted_count, edit_count,
                heavy_edit_count, learning_modal_shown, learning_confirmed, failure_count,
                fallback_count
           FROM runtime_telemetry_daily
          WHERE org_id = $1 AND account_id = $2
          ORDER BY event_date DESC, mode ASC
          LIMIT 30",
    )
    .bind(org_id)
    .bind(account_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let uploads: Vec<(
        DateTime<Utc>,
        Option<String>,
        Option<String>,
        i32,
        i32,
        i32,
        i32,
    )> = sqlx::query_as(
        "SELECT received_at, device_id, client_version, run_count, rollup_count,
                accepted_count, rejected_count
           FROM runtime_telemetry_uploads
          WHERE org_id = $1 AND account_id = $2
          ORDER BY received_at DESC
          LIMIT 20",
    )
    .bind(org_id)
    .bind(account_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let mut speech_json = speech_breakdown_json(&speech_rows);
    if let Some(obj) = speech_json.as_object_mut() {
        obj.insert("latency_by_model".into(), json!(speech_latency));
    }

    Ok(Json(json!({
        "window_days": days,
        "member": {
            "account_id": account_id,
            "email": email,
            "lark_name": lark_name,
            "lark_department": lark_department,
            "role": member_role,
            "auth_source": auth_source,
            "lark_connected": lark_connected,
            "desktop_active": desktop_active,
        },
        "summary": {
            "runs": totals.run_count,
            "audio_minutes": (totals.audio_seconds / 60.0 * 10.0).round() / 10.0,
            "word_count": totals.word_count,
            "char_count": totals.char_count,
        },
        "quality": quality_json(&totals),
        "quality_counts": {
            "accepted_as_is": totals.accepted,
            "edit_detected": totals.edits,
            "heavy_edit": totals.heavy_edits,
            "deleted_entire_output": totals.deleted_entire_output,
            "re_recorded_quickly": totals.re_recorded_quickly,
            "failures": totals.failures,
        },
        "learning": {
            "learning_candidate": totals.learning_candidates,
            "learning_modal_shown": totals.learning_modal_shown,
            "learning_confirmed": totals.learning_confirmed,
            "learning_dismissed": totals.learning_dismissed,
            "server_learning_saved": totals.learning_saved,
            "server_learning_blocked": totals.server_learning_blocked,
        },
        "latency_ms": latency_json(&latency),
        "speech": speech_json,
        "by_mode": by_mode.iter().map(|(m, c)| json!({"mode": m, "count": c})).collect::<Vec<_>>(),
        "by_target_app": by_app.iter().map(|(a, c)| json!({"target_app": a, "count": c})).collect::<Vec<_>>(),
        "content_flags": {
            "has_numbers": flags.0,
            "has_currency": flags.1,
            "has_percent": flags.2,
            "has_email": flags.3,
            "has_url": flags.4,
            "has_code_like_terms": flags.5,
            "mixed_language": flags.6,
            "protected_term_hit": flags.7,
        },
        "daily_rollups": daily_rollups.iter().map(|(d, mode, runs, audio, acc, ed, heavy, lm, lc, fail, fb)| {
            json!({
                "event_date": d.to_string(),
                "mode": mode,
                "run_count": runs,
                "audio_seconds": audio,
                "accepted_count": acc,
                "edit_count": ed,
                "heavy_edit_count": heavy,
                "learning_modal_shown": lm,
                "learning_confirmed": lc,
                "failure_count": fail,
                "fallback_count": fb,
            })
        }).collect::<Vec<_>>(),
        "uploads": uploads.iter().map(|(at, dev, ver, rc, ruc, acc, rej)| {
            json!({
                "received_at": at,
                "device_id": dev,
                "client_version": ver,
                "run_count": rc,
                "rollup_count": ruc,
                "accepted_count": acc,
                "rejected_count": rej,
            })
        }).collect::<Vec<_>>(),
    })))
}

pub async fn user_runs(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Path((org_id, account_id)): Path<(Uuid, Uuid)>,
    Query(q): Query<UserRunsQuery>,
) -> Result<Json<Value>, StatusCode> {
    let (_, role) = tenant::ensure_path_org_active(&state, &user, &headers, org_id)
        .await
        .map_err(|_| StatusCode::FORBIDDEN)?;
    require_org_viewer(&role)?;
    ensure_org_account_member(&state.db, org_id, account_id).await?;

    let (days, since) = window_days(q.days);
    let limit = q.limit.unwrap_or(25).clamp(1, 100) as i64;
    let offset = q.offset.unwrap_or(0).max(0) as i64;
    let mode_filter = q
        .mode
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "all");

    let total: i64 = if let Some(mode) = mode_filter {
        sqlx::query_scalar(
            "SELECT COUNT(*)::bigint FROM runtime_telemetry_runs
              WHERE org_id = $1 AND account_id = $2 AND event_at >= $3 AND mode = $4",
        )
        .bind(org_id)
        .bind(account_id)
        .bind(since)
        .bind(mode)
        .fetch_one(&state.db)
        .await
        .unwrap_or(0)
    } else {
        sqlx::query_scalar(
            "SELECT COUNT(*)::bigint FROM runtime_telemetry_runs
              WHERE org_id = $1 AND account_id = $2 AND event_at >= $3",
        )
        .bind(org_id)
        .bind(account_id)
        .bind(since)
        .fetch_one(&state.db)
        .await
        .unwrap_or(0)
    };

    let runs: Vec<TelemetryRunRow> = if let Some(mode) = mode_filter {
        sqlx::query_as(
            "SELECT run_id, recording_id, device_id, mode, target_app, platform, app_version,
                    machine_class, audio_seconds, word_count, char_count, transcribe_ms, embed_ms,
                    polish_ms, total_ms, paste_ms, success, error_code, used_clipboard_fallback,
                    speech_model, speech_path, edit_detected, edit_bucket, edit_distance_chars, edit_distance_words,
                    accepted_as_is, deleted_entire_output, re_recorded_quickly, learning_candidate,
                    learning_modal_shown, learning_confirmed, learning_dismissed, server_learning_saved,
                    server_learning_blocked, has_numbers, has_currency, has_percent, has_email, has_url,
                    has_code_like_terms, mixed_language, protected_term_hit, client_version, event_at,
                    received_at
               FROM runtime_telemetry_runs
              WHERE org_id = $1 AND account_id = $2 AND event_at >= $3 AND mode = $4
              ORDER BY event_at DESC
              LIMIT $5 OFFSET $6",
        )
        .bind(org_id)
        .bind(account_id)
        .bind(since)
        .bind(mode)
        .bind(limit)
        .bind(offset)
        .fetch_all(&state.db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    } else {
        sqlx::query_as(
            "SELECT run_id, recording_id, device_id, mode, target_app, platform, app_version,
                    machine_class, audio_seconds, word_count, char_count, transcribe_ms, embed_ms,
                    polish_ms, total_ms, paste_ms, success, error_code, used_clipboard_fallback,
                    speech_model, speech_path, edit_detected, edit_bucket, edit_distance_chars, edit_distance_words,
                    accepted_as_is, deleted_entire_output, re_recorded_quickly, learning_candidate,
                    learning_modal_shown, learning_confirmed, learning_dismissed, server_learning_saved,
                    server_learning_blocked, has_numbers, has_currency, has_percent, has_email, has_url,
                    has_code_like_terms, mixed_language, protected_term_hit, client_version, event_at,
                    received_at
               FROM runtime_telemetry_runs
              WHERE org_id = $1 AND account_id = $2 AND event_at >= $3
              ORDER BY event_at DESC
              LIMIT $4 OFFSET $5",
        )
        .bind(org_id)
        .bind(account_id)
        .bind(since)
        .bind(limit)
        .bind(offset)
        .fetch_all(&state.db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };

    let run_json: Vec<Value> = runs.into_iter().map(run_row_to_json).collect();

    Ok(Json(json!({
        "window_days": days,
        "total": total,
        "limit": limit,
        "offset": offset,
        "runs": run_json,
    })))
}

#[derive(sqlx::FromRow)]
struct TelemetryRunRow {
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
    speech_model: Option<String>,
    speech_path: Option<String>,
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
    client_version: Option<String>,
    event_at: DateTime<Utc>,
    received_at: DateTime<Utc>,
}

#[derive(Serialize)]
struct RunContentFlags {
    has_numbers: bool,
    has_currency: bool,
    has_percent: bool,
    has_email: bool,
    has_url: bool,
    has_code_like_terms: bool,
    mixed_language: bool,
    protected_term_hit: bool,
}

#[derive(Serialize)]
struct TelemetryRunOut {
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
    speech_model: Option<String>,
    speech_path: Option<String>,
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
    content_flags: RunContentFlags,
    client_version: Option<String>,
    event_at: DateTime<Utc>,
    received_at: DateTime<Utc>,
}

fn run_row_to_json(r: TelemetryRunRow) -> Value {
    serde_json::to_value(TelemetryRunOut {
        run_id: r.run_id,
        recording_id: r.recording_id,
        device_id: r.device_id,
        mode: r.mode,
        target_app: r.target_app,
        platform: r.platform,
        app_version: r.app_version,
        machine_class: r.machine_class,
        audio_seconds: r.audio_seconds,
        word_count: r.word_count,
        char_count: r.char_count,
        transcribe_ms: r.transcribe_ms,
        embed_ms: r.embed_ms,
        polish_ms: r.polish_ms,
        total_ms: r.total_ms,
        paste_ms: r.paste_ms,
        success: r.success,
        error_code: r.error_code,
        used_clipboard_fallback: r.used_clipboard_fallback,
        speech_model: r.speech_model,
        speech_path: r.speech_path,
        edit_detected: r.edit_detected,
        edit_bucket: r.edit_bucket,
        edit_distance_chars: r.edit_distance_chars,
        edit_distance_words: r.edit_distance_words,
        accepted_as_is: r.accepted_as_is,
        deleted_entire_output: r.deleted_entire_output,
        re_recorded_quickly: r.re_recorded_quickly,
        learning_candidate: r.learning_candidate,
        learning_modal_shown: r.learning_modal_shown,
        learning_confirmed: r.learning_confirmed,
        learning_dismissed: r.learning_dismissed,
        server_learning_saved: r.server_learning_saved,
        server_learning_blocked: r.server_learning_blocked,
        content_flags: RunContentFlags {
            has_numbers: r.has_numbers,
            has_currency: r.has_currency,
            has_percent: r.has_percent,
            has_email: r.has_email,
            has_url: r.has_url,
            has_code_like_terms: r.has_code_like_terms,
            mixed_language: r.mixed_language,
            protected_term_hit: r.protected_term_hit,
        },
        client_version: r.client_version,
        event_at: r.event_at,
        received_at: r.received_at,
    })
    .unwrap_or(Value::Null)
}

pub async fn user_memory(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Path((org_id, account_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Value>, StatusCode> {
    let (_, role) = tenant::ensure_path_org_active(&state, &user, &headers, org_id)
        .await
        .map_err(|_| StatusCode::FORBIDDEN)?;
    require_org_viewer(&role)?;
    ensure_org_account_member(&state.db, org_id, account_id).await?;

    let hygiene: Option<(Option<DateTime<Utc>>, Option<DateTime<Utc>>, i32)> = sqlx::query_as(
        "SELECT memory_dirty_at, last_hygiene_at, hygiene_version
           FROM personal_memory_hygiene_state
          WHERE account_id = $1",
    )
    .bind(account_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let vocab: Vec<(String, String, f64, i32, String, String)> = sqlx::query_as(
        "SELECT term, term_type, weight, positive_count, status, source
           FROM personal_vocab_terms
          WHERE account_id = $1
          ORDER BY positive_count DESC, updated_at DESC
          LIMIT 100",
    )
    .bind(account_id)
    .fetch_all(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let aliases: Vec<(String, String, f64, i32, String, String, Option<String>)> = sqlx::query_as(
        "SELECT transcript_form, correct_form, weight, positive_count, status, safety_status,
                learned_speech_model
           FROM personal_stt_replacements
          WHERE account_id = $1
          ORDER BY positive_count DESC, updated_at DESC
          LIMIT 100",
    )
    .bind(account_id)
    .fetch_all(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let policies: Vec<(String, String, String, i32, i32, String)> = sqlx::query_as(
        "SELECT variant_form, correct_form, edit_type, positive_count, negative_count, status
           FROM personal_edit_policy_rules
          WHERE account_id = $1
          ORDER BY positive_count DESC, updated_at DESC
          LIMIT 100",
    )
    .bind(account_id)
    .fetch_all(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let audit: Vec<(
        DateTime<Utc>,
        String,
        Option<String>,
        Option<String>,
        String,
        String,
        Option<String>,
    )> = sqlx::query_as(
        "SELECT created_at, action, heard, correct, verdict, reason, model
           FROM alias_safety_audit
          WHERE account_id = $1
          ORDER BY created_at DESC
          LIMIT 50",
    )
    .bind(account_id)
    .fetch_all(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let (memory_dirty_at, last_hygiene_at, hygiene_version) = hygiene.unwrap_or((None, None, 1));

    let prompt_profile_latest: Option<(
        String,
        String,
        i32,
        String,
        Option<i64>,
        Option<Uuid>,
        DateTime<Utc>,
    )> = sqlx::query_as(
        "SELECT profile_source, profile_markdown, profile_chars, profile_hash,
                client_profile_version, last_run_id, updated_at
           FROM runtime_prompt_profile_latest
          WHERE account_id = $1 AND org_scope = $2",
    )
    .bind(account_id)
    .bind(org_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let prompt_profile_latest = if prompt_profile_latest.is_some() {
        prompt_profile_latest
    } else {
        sqlx::query_as(
            "SELECT profile_source, profile_markdown, profile_chars, profile_hash,
                    client_profile_version, last_run_id, updated_at
               FROM runtime_prompt_profile_latest
              WHERE account_id = $1 AND org_scope = $2",
        )
        .bind(account_id)
        .bind(crate::profile::alias_safety::global_org_scope())
        .fetch_optional(&state.db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };

    let server_learned_profile =
        crate::profile::store::get_profile_with_fallback(&state.db, account_id, org_id)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(json!({
        "hygiene": {
            "memory_dirty_at": memory_dirty_at,
            "last_hygiene_at": last_hygiene_at,
            "hygiene_version": hygiene_version,
            "pending_review": memory_dirty_at.is_some(),
        },
        "vocab_terms": vocab.iter().map(|(term, term_type, weight, pos, status, source)| {
            json!({
                "term": term,
                "term_type": term_type,
                "weight": weight,
                "positive_count": pos,
                "status": status,
                "source": source,
            })
        }).collect::<Vec<_>>(),
        "aliases": aliases.iter().map(|(heard, correct, weight, pos, status, safety, speech_model)| {
            json!({
                "transcript_form": heard,
                "correct_form": correct,
                "weight": weight,
                "positive_count": pos,
                "status": status,
                "safety_status": safety,
                "learned_speech_model": speech_model,
            })
        }).collect::<Vec<_>>(),
        "edit_policies": policies.iter().map(|(variant, correct, edit_type, pos, neg, status)| {
            json!({
                "variant_form": variant,
                "correct_form": correct,
                "edit_type": edit_type,
                "positive_count": pos,
                "negative_count": neg,
                "status": status,
            })
        }).collect::<Vec<_>>(),
        "audit_log": audit.iter().map(|(at, action, heard, correct, verdict, reason, model)| {
            json!({
                "created_at": at,
                "action": action,
                "heard": heard,
                "correct": correct,
                "verdict": verdict,
                "reason": reason,
                "model": model,
            })
        }).collect::<Vec<_>>(),
        "prompt_profile_latest": prompt_profile_latest.map(|(source, markdown, chars, hash, version, run_id, updated_at)| {
            json!({
                "profile_source": source,
                "profile_markdown": markdown,
                "profile_chars": chars,
                "profile_hash": hash,
                "client_profile_version": version,
                "last_run_id": run_id,
                "updated_at": updated_at,
            })
        }),
        "server_learned_profile": server_learned_profile.map(|row| {
            json!({
                "profile_markdown": row.profile_markdown,
                "version": row.version,
                "status": row.status,
                "updated_at": row.updated_at,
            })
        }),
    })))
}

fn ms_to_datetime(ms: i64) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp_millis(ms)
}

#[cfg(test)]
mod tests {
    use super::telemetry_rate;

    #[test]
    fn rate_zero_total() {
        assert!((telemetry_rate(5, 0) - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn rate_normal() {
        assert!((telemetry_rate(1, 4) - 0.25).abs() < f64::EPSILON);
    }
}
