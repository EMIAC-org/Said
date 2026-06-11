//! Local telemetry outbox — per-run summaries and daily rollups.

use rusqlite::params;
use serde::{Deserialize, Serialize};

use super::DbPool;

pub const STATUS_PENDING: &str = "pending";
pub const STATUS_READY: &str = "ready";
pub const STATUS_UPLOADED: &str = "uploaded";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RunSummaryPatch {
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
    pub success: Option<bool>,
    pub error_code: Option<String>,
    pub used_clipboard_fallback: Option<bool>,
    pub used_ws_pretranscript: Option<bool>,
    pub used_http_stt_fallback: Option<bool>,
    pub stt_provider: Option<String>,
    pub stt_model: Option<String>,
    pub stt_path: Option<String>,
    pub edit_detected: Option<bool>,
    pub edit_bucket: Option<String>,
    pub edit_distance_chars: Option<i32>,
    pub edit_distance_words: Option<i32>,
    pub accepted_as_is: Option<bool>,
    pub deleted_entire_output: Option<bool>,
    pub re_recorded_quickly: Option<bool>,
    pub learning_candidate: Option<bool>,
    pub learning_modal_shown: Option<bool>,
    pub learning_confirmed: Option<bool>,
    pub learning_dismissed: Option<bool>,
    pub server_learning_saved: Option<bool>,
    pub server_learning_blocked: Option<bool>,
    pub has_numbers: Option<bool>,
    pub has_currency: Option<bool>,
    pub has_percent: Option<bool>,
    pub has_email: Option<bool>,
    pub has_url: Option<bool>,
    pub has_code_like_terms: Option<bool>,
    pub mixed_language: Option<bool>,
    pub protected_term_hit: Option<bool>,
    #[serde(default)]
    pub finalize: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunSummaryRow {
    pub run_id: String,
    pub recording_id: Option<String>,
    pub user_id: String,
    pub device_id: Option<String>,
    pub mode: String,
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
    pub success: bool,
    pub error_code: Option<String>,
    pub used_clipboard_fallback: bool,
    pub used_ws_pretranscript: bool,
    pub used_http_stt_fallback: bool,
    pub stt_provider: Option<String>,
    pub stt_model: Option<String>,
    pub stt_path: Option<String>,
    pub edit_detected: bool,
    pub edit_bucket: String,
    pub edit_distance_chars: Option<i32>,
    pub edit_distance_words: Option<i32>,
    pub accepted_as_is: bool,
    pub deleted_entire_output: bool,
    pub re_recorded_quickly: bool,
    pub learning_candidate: bool,
    pub learning_modal_shown: bool,
    pub learning_confirmed: bool,
    pub learning_dismissed: bool,
    pub server_learning_saved: bool,
    pub server_learning_blocked: bool,
    pub has_numbers: bool,
    pub has_currency: bool,
    pub has_percent: bool,
    pub has_email: bool,
    pub has_url: bool,
    pub has_code_like_terms: bool,
    pub mixed_language: bool,
    pub protected_term_hit: bool,
    pub status: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DailyRollupRow {
    pub event_date: String,
    pub mode: String,
    pub run_count: i32,
    pub audio_seconds: f64,
    pub accepted_count: i32,
    pub edit_count: i32,
    pub heavy_edit_count: i32,
    pub learning_modal_shown: i32,
    pub learning_confirmed: i32,
    pub failure_count: i32,
    pub fallback_count: i32,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn upsert_run_start(
    pool: &DbPool,
    user_id: &str,
    run_id: &str,
    patch: &RunSummaryPatch,
) -> Result<(), String> {
    let now = now_ms();
    let conn = pool.get().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO telemetry_run_summaries
            (run_id, user_id, device_id, mode, target_app, platform, app_version, machine_class,
             status, created_at_ms, updated_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending', ?9, ?9)
         ON CONFLICT(run_id) DO UPDATE SET
            updated_at_ms = excluded.updated_at_ms",
        params![
            run_id,
            user_id,
            patch.device_id,
            patch.mode.as_deref().unwrap_or("normal_voice"),
            patch.target_app,
            patch.platform,
            patch.app_version,
            patch.machine_class,
            now,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn patch_run(
    pool: &DbPool,
    user_id: &str,
    run_id: &str,
    patch: &RunSummaryPatch,
) -> Result<(), String> {
    let now = now_ms();
    let exists = load_run(pool, user_id, run_id).ok();
    if exists.is_none() {
        upsert_run_start(pool, user_id, run_id, patch)?;
    }
    let mut row = exists.unwrap_or_else(|| RunSummaryRow {
        run_id: run_id.to_string(),
        recording_id: None,
        user_id: user_id.to_string(),
        device_id: None,
        mode: patch.mode.clone().unwrap_or_else(|| "normal_voice".into()),
        target_app: None,
        platform: None,
        app_version: None,
        machine_class: None,
        audio_seconds: None,
        word_count: None,
        char_count: None,
        transcribe_ms: None,
        embed_ms: None,
        polish_ms: None,
        total_ms: None,
        paste_ms: None,
        success: false,
        error_code: None,
        used_clipboard_fallback: false,
        used_ws_pretranscript: false,
        used_http_stt_fallback: false,
        stt_provider: None,
        stt_model: None,
        stt_path: None,
        edit_detected: false,
        edit_bucket: "none".into(),
        edit_distance_chars: None,
        edit_distance_words: None,
        accepted_as_is: false,
        deleted_entire_output: false,
        re_recorded_quickly: false,
        learning_candidate: false,
        learning_modal_shown: false,
        learning_confirmed: false,
        learning_dismissed: false,
        server_learning_saved: false,
        server_learning_blocked: false,
        has_numbers: false,
        has_currency: false,
        has_percent: false,
        has_email: false,
        has_url: false,
        has_code_like_terms: false,
        mixed_language: false,
        protected_term_hit: false,
        status: STATUS_PENDING.into(),
        created_at_ms: now,
        updated_at_ms: now,
    });

    macro_rules! merge_opt_str {
        ($field:ident) => {
            if let Some(v) = patch.$field.clone() {
                row.$field = Some(v);
            }
        };
    }
    macro_rules! merge_opt_num {
        ($field:ident) => {
            if let Some(v) = patch.$field {
                row.$field = Some(v);
            }
        };
    }
    macro_rules! merge_bool {
        ($field:ident) => {
            if let Some(v) = patch.$field {
                row.$field = v;
            }
        };
    }

    merge_opt_str!(recording_id);
    merge_opt_str!(device_id);
    if let Some(v) = &patch.mode {
        row.mode = v.clone();
    }
    merge_opt_str!(target_app);
    merge_opt_str!(platform);
    merge_opt_str!(app_version);
    merge_opt_str!(machine_class);
    merge_opt_num!(audio_seconds);
    merge_opt_num!(word_count);
    merge_opt_num!(char_count);
    merge_opt_num!(transcribe_ms);
    merge_opt_num!(embed_ms);
    merge_opt_num!(polish_ms);
    merge_opt_num!(total_ms);
    merge_opt_num!(paste_ms);
    merge_bool!(success);
    merge_opt_str!(error_code);
    merge_bool!(used_clipboard_fallback);
    merge_bool!(used_ws_pretranscript);
    merge_bool!(used_http_stt_fallback);
    merge_opt_str!(stt_provider);
    merge_opt_str!(stt_model);
    merge_opt_str!(stt_path);
    merge_bool!(edit_detected);
    if let Some(v) = &patch.edit_bucket {
        row.edit_bucket = v.clone();
    }
    merge_opt_num!(edit_distance_chars);
    merge_opt_num!(edit_distance_words);
    merge_bool!(accepted_as_is);
    merge_bool!(deleted_entire_output);
    merge_bool!(re_recorded_quickly);
    merge_bool!(learning_candidate);
    merge_bool!(learning_modal_shown);
    merge_bool!(learning_confirmed);
    merge_bool!(learning_dismissed);
    merge_bool!(server_learning_saved);
    merge_bool!(server_learning_blocked);
    merge_bool!(has_numbers);
    merge_bool!(has_currency);
    merge_bool!(has_percent);
    merge_bool!(has_email);
    merge_bool!(has_url);
    merge_bool!(has_code_like_terms);
    merge_bool!(mixed_language);
    merge_bool!(protected_term_hit);

    row.updated_at_ms = now;
    if patch.finalize {
        row.status = STATUS_READY.into();
    }

    write_run(pool, &row, if patch.finalize { Some(now) } else { None })?;

    if patch.finalize {
        bump_daily_rollup(pool, user_id, &row);
        bump_upload_counter(pool, user_id);
    }

    Ok(())
}

fn write_run(pool: &DbPool, row: &RunSummaryRow, ready_at_ms: Option<i64>) -> Result<(), String> {
    let conn = pool.get().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO telemetry_run_summaries (
            run_id, recording_id, user_id, device_id, mode, target_app, platform, app_version,
            machine_class, audio_seconds, word_count, char_count, transcribe_ms, embed_ms,
            polish_ms, total_ms, paste_ms, success, error_code, used_clipboard_fallback,
            used_ws_pretranscript, used_http_stt_fallback, stt_provider, stt_model, stt_path,
            edit_detected, edit_bucket,
            edit_distance_chars, edit_distance_words, accepted_as_is, deleted_entire_output,
            re_recorded_quickly, learning_candidate, learning_modal_shown, learning_confirmed,
            learning_dismissed, server_learning_saved, server_learning_blocked, has_numbers,
            has_currency, has_percent, has_email, has_url, has_code_like_terms, mixed_language,
            protected_term_hit, status, created_at_ms, updated_at_ms, ready_at_ms
        ) VALUES (
            ?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27,
            ?28,?29,?30,?31,?32,?33,?34,?35,?36,?37,?38,?39,?40,?41,?42,?43,?44,?45,?46,?47,?48,?49,?50
        ) ON CONFLICT(run_id) DO UPDATE SET
            recording_id=excluded.recording_id, device_id=excluded.device_id, mode=excluded.mode,
            target_app=excluded.target_app, platform=excluded.platform, app_version=excluded.app_version,
            machine_class=excluded.machine_class, audio_seconds=excluded.audio_seconds,
            word_count=excluded.word_count, char_count=excluded.char_count,
            transcribe_ms=excluded.transcribe_ms, embed_ms=excluded.embed_ms,
            polish_ms=excluded.polish_ms, total_ms=excluded.total_ms, paste_ms=excluded.paste_ms,
            success=excluded.success, error_code=excluded.error_code,
            used_clipboard_fallback=excluded.used_clipboard_fallback,
            used_ws_pretranscript=excluded.used_ws_pretranscript,
            used_http_stt_fallback=excluded.used_http_stt_fallback,
            stt_provider=excluded.stt_provider, stt_model=excluded.stt_model,
            stt_path=excluded.stt_path,
            edit_detected=excluded.edit_detected, edit_bucket=excluded.edit_bucket,
            edit_distance_chars=excluded.edit_distance_chars,
            edit_distance_words=excluded.edit_distance_words,
            accepted_as_is=excluded.accepted_as_is,
            deleted_entire_output=excluded.deleted_entire_output,
            re_recorded_quickly=excluded.re_recorded_quickly,
            learning_candidate=excluded.learning_candidate,
            learning_modal_shown=excluded.learning_modal_shown,
            learning_confirmed=excluded.learning_confirmed,
            learning_dismissed=excluded.learning_dismissed,
            server_learning_saved=excluded.server_learning_saved,
            server_learning_blocked=excluded.server_learning_blocked,
            has_numbers=excluded.has_numbers, has_currency=excluded.has_currency,
            has_percent=excluded.has_percent, has_email=excluded.has_email,
            has_url=excluded.has_url, has_code_like_terms=excluded.has_code_like_terms,
            mixed_language=excluded.mixed_language, protected_term_hit=excluded.protected_term_hit,
            status=excluded.status, updated_at_ms=excluded.updated_at_ms,
            ready_at_ms=COALESCE(excluded.ready_at_ms, telemetry_run_summaries.ready_at_ms)",
        params![
            row.run_id,
            row.recording_id,
            row.user_id,
            row.device_id,
            row.mode,
            row.target_app,
            row.platform,
            row.app_version,
            row.machine_class,
            row.audio_seconds,
            row.word_count,
            row.char_count,
            row.transcribe_ms,
            row.embed_ms,
            row.polish_ms,
            row.total_ms,
            row.paste_ms,
            i32::from(row.success),
            row.error_code,
            i32::from(row.used_clipboard_fallback),
            i32::from(row.used_ws_pretranscript),
            i32::from(row.used_http_stt_fallback),
            row.stt_provider,
            row.stt_model,
            row.stt_path,
            i32::from(row.edit_detected),
            row.edit_bucket,
            row.edit_distance_chars,
            row.edit_distance_words,
            i32::from(row.accepted_as_is),
            i32::from(row.deleted_entire_output),
            i32::from(row.re_recorded_quickly),
            i32::from(row.learning_candidate),
            i32::from(row.learning_modal_shown),
            i32::from(row.learning_confirmed),
            i32::from(row.learning_dismissed),
            i32::from(row.server_learning_saved),
            i32::from(row.server_learning_blocked),
            i32::from(row.has_numbers),
            i32::from(row.has_currency),
            i32::from(row.has_percent),
            i32::from(row.has_email),
            i32::from(row.has_url),
            i32::from(row.has_code_like_terms),
            i32::from(row.mixed_language),
            i32::from(row.protected_term_hit),
            row.status,
            row.created_at_ms,
            row.updated_at_ms,
            ready_at_ms,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn bump_upload_counter(pool: &DbPool, user_id: &str) {
    if let Ok(conn) = pool.get() {
        let _ = conn.execute(
            "INSERT INTO telemetry_upload_state (user_id, completed_since_upload, pending_run_count)
             VALUES (?1, 1, 1)
             ON CONFLICT(user_id) DO UPDATE SET
                completed_since_upload = telemetry_upload_state.completed_since_upload + 1,
                pending_run_count = telemetry_upload_state.pending_run_count + 1",
            params![user_id],
        );
    }
}

fn bump_daily_rollup(pool: &DbPool, user_id: &str, row: &RunSummaryRow) {
    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let now = now_ms();
    let heavy = matches!(
        row.edit_bucket.as_str(),
        "medium" | "heavy" | "full_replace"
    );
    let fallback = row.used_clipboard_fallback || row.used_http_stt_fallback;
    let conn = match pool.get() {
        Ok(c) => c,
        Err(_) => return,
    };
    for mode in [row.mode.as_str(), "all"] {
        let _ = conn.execute(
            "INSERT INTO telemetry_daily_rollups
                (user_id, event_date, mode, run_count, audio_seconds, accepted_count, edit_count,
                 heavy_edit_count, learning_modal_shown, learning_confirmed, failure_count,
                 fallback_count, updated_at_ms)
             VALUES (?1, ?2, ?3, 1, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT(user_id, event_date, mode) DO UPDATE SET
                run_count = telemetry_daily_rollups.run_count + 1,
                audio_seconds = telemetry_daily_rollups.audio_seconds + excluded.audio_seconds,
                accepted_count = telemetry_daily_rollups.accepted_count + excluded.accepted_count,
                edit_count = telemetry_daily_rollups.edit_count + excluded.edit_count,
                heavy_edit_count = telemetry_daily_rollups.heavy_edit_count + excluded.heavy_edit_count,
                learning_modal_shown = telemetry_daily_rollups.learning_modal_shown + excluded.learning_modal_shown,
                learning_confirmed = telemetry_daily_rollups.learning_confirmed + excluded.learning_confirmed,
                failure_count = telemetry_daily_rollups.failure_count + excluded.failure_count,
                fallback_count = telemetry_daily_rollups.fallback_count + excluded.fallback_count,
                updated_at_ms = excluded.updated_at_ms",
            params![
                user_id,
                date,
                mode,
                row.audio_seconds.unwrap_or(0.0),
                if row.accepted_as_is { 1 } else { 0 },
                if row.edit_detected { 1 } else { 0 },
                if heavy { 1 } else { 0 },
                if row.learning_modal_shown { 1 } else { 0 },
                if row.learning_confirmed { 1 } else { 0 },
                if !row.success { 1 } else { 0 },
                if fallback { 1 } else { 0 },
                now,
            ],
        );
    }
}

pub fn finalize_stale_runs(
    pool: &DbPool,
    user_id: &str,
    stale_after_ms: i64,
) -> Result<usize, String> {
    let cutoff = now_ms() - stale_after_ms;
    let conn = pool.get().map_err(|e| e.to_string())?;
    let updated = conn
        .execute(
            "UPDATE telemetry_run_summaries
                SET status = 'ready',
                    ready_at_ms = updated_at_ms,
                    accepted_as_is = CASE
                        WHEN success = 1 AND edit_detected = 0 THEN 1
                        ELSE accepted_as_is
                    END
              WHERE user_id = ?1 AND status = 'pending' AND updated_at_ms < ?2",
            params![user_id, cutoff],
        )
        .map_err(|e| e.to_string())?;
    Ok(updated)
}

pub fn list_ready_runs(
    pool: &DbPool,
    user_id: &str,
    limit: usize,
) -> Result<Vec<RunSummaryRow>, String> {
    let conn = pool.get().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT run_id, recording_id, user_id, device_id, mode, target_app, platform, app_version,
                    machine_class, audio_seconds, word_count, char_count, transcribe_ms, embed_ms,
                    polish_ms, total_ms, paste_ms, success, error_code, used_clipboard_fallback,
                    used_ws_pretranscript, used_http_stt_fallback, stt_provider, stt_model, stt_path,
                    edit_detected, edit_bucket, edit_distance_chars, edit_distance_words,
                    accepted_as_is, deleted_entire_output, re_recorded_quickly, learning_candidate,
                    learning_modal_shown, learning_confirmed, learning_dismissed, server_learning_saved,
                    server_learning_blocked, has_numbers, has_currency, has_percent, has_email, has_url,
                    has_code_like_terms, mixed_language, protected_term_hit, status, created_at_ms,
                    updated_at_ms
               FROM telemetry_run_summaries
              WHERE user_id = ?1 AND status = 'ready'
              ORDER BY updated_at_ms ASC
              LIMIT ?2",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![user_id, limit as i64], map_run_row)
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

pub fn list_ready_rollups(
    pool: &DbPool,
    user_id: &str,
    limit: usize,
) -> Result<Vec<DailyRollupRow>, String> {
    let conn = pool.get().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT event_date, mode, run_count, audio_seconds, accepted_count, edit_count,
                    heavy_edit_count, learning_modal_shown, learning_confirmed, failure_count,
                    fallback_count
               FROM telemetry_daily_rollups
              WHERE user_id = ?1
              ORDER BY event_date DESC
              LIMIT ?2",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![user_id, limit as i64], |row| {
            Ok(DailyRollupRow {
                event_date: row.get(0)?,
                mode: row.get(1)?,
                run_count: row.get(2)?,
                audio_seconds: row.get(3)?,
                accepted_count: row.get(4)?,
                edit_count: row.get(5)?,
                heavy_edit_count: row.get(6)?,
                learning_modal_shown: row.get(7)?,
                learning_confirmed: row.get(8)?,
                failure_count: row.get(9)?,
                fallback_count: row.get(10)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

pub fn mark_runs_uploaded(pool: &DbPool, user_id: &str, run_ids: &[String]) -> Result<(), String> {
    if run_ids.is_empty() {
        return Ok(());
    }
    let now = now_ms();
    let conn = pool.get().map_err(|e| e.to_string())?;
    for run_id in run_ids {
        let _ = conn.execute(
            "UPDATE telemetry_run_summaries
                SET status = 'uploaded', uploaded_at_ms = ?1
              WHERE run_id = ?2 AND user_id = ?3 AND status = 'ready'",
            params![now, run_id, user_id],
        );
    }
    let _ = conn.execute(
        "UPDATE telemetry_upload_state
            SET last_upload_at_ms = ?1, completed_since_upload = 0, pending_run_count = (
                SELECT COUNT(*) FROM telemetry_run_summaries WHERE user_id = ?2 AND status = 'ready'
            )
          WHERE user_id = ?2",
        params![now, user_id],
    );
    Ok(())
}

fn load_run(pool: &DbPool, user_id: &str, run_id: &str) -> Result<RunSummaryRow, String> {
    let conn = pool.get().map_err(|e| e.to_string())?;
    conn.query_row(
        "SELECT run_id, recording_id, user_id, device_id, mode, target_app, platform, app_version,
                machine_class, audio_seconds, word_count, char_count, transcribe_ms, embed_ms,
                polish_ms, total_ms, paste_ms, success, error_code, used_clipboard_fallback,
                used_ws_pretranscript, used_http_stt_fallback, stt_provider, stt_model, stt_path,
                edit_detected, edit_bucket, edit_distance_chars, edit_distance_words, accepted_as_is,
                deleted_entire_output, re_recorded_quickly, learning_candidate, learning_modal_shown,
                learning_confirmed, learning_dismissed, server_learning_saved, server_learning_blocked,
                has_numbers, has_currency, has_percent, has_email, has_url, has_code_like_terms,
                mixed_language, protected_term_hit, status, created_at_ms, updated_at_ms
           FROM telemetry_run_summaries WHERE run_id = ?1 AND user_id = ?2",
        params![run_id, user_id],
        map_run_row,
    )
    .map_err(|e| e.to_string())
}

fn map_run_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RunSummaryRow> {
    Ok(RunSummaryRow {
        run_id: row.get(0)?,
        recording_id: row.get(1)?,
        user_id: row.get(2)?,
        device_id: row.get(3)?,
        mode: row.get(4)?,
        target_app: row.get(5)?,
        platform: row.get(6)?,
        app_version: row.get(7)?,
        machine_class: row.get(8)?,
        audio_seconds: row.get(9)?,
        word_count: row.get(10)?,
        char_count: row.get(11)?,
        transcribe_ms: row.get(12)?,
        embed_ms: row.get(13)?,
        polish_ms: row.get(14)?,
        total_ms: row.get(15)?,
        paste_ms: row.get(16)?,
        success: row.get::<_, i32>(17)? != 0,
        error_code: row.get(18)?,
        used_clipboard_fallback: row.get::<_, i32>(19)? != 0,
        used_ws_pretranscript: row.get::<_, i32>(20)? != 0,
        used_http_stt_fallback: row.get::<_, i32>(21)? != 0,
        stt_provider: row.get(22)?,
        stt_model: row.get(23)?,
        stt_path: row.get(24)?,
        edit_detected: row.get::<_, i32>(25)? != 0,
        edit_bucket: row.get(26)?,
        edit_distance_chars: row.get(27)?,
        edit_distance_words: row.get(28)?,
        accepted_as_is: row.get::<_, i32>(29)? != 0,
        deleted_entire_output: row.get::<_, i32>(30)? != 0,
        re_recorded_quickly: row.get::<_, i32>(31)? != 0,
        learning_candidate: row.get::<_, i32>(32)? != 0,
        learning_modal_shown: row.get::<_, i32>(33)? != 0,
        learning_confirmed: row.get::<_, i32>(34)? != 0,
        learning_dismissed: row.get::<_, i32>(35)? != 0,
        server_learning_saved: row.get::<_, i32>(36)? != 0,
        server_learning_blocked: row.get::<_, i32>(37)? != 0,
        has_numbers: row.get::<_, i32>(38)? != 0,
        has_currency: row.get::<_, i32>(39)? != 0,
        has_percent: row.get::<_, i32>(40)? != 0,
        has_email: row.get::<_, i32>(41)? != 0,
        has_url: row.get::<_, i32>(42)? != 0,
        has_code_like_terms: row.get::<_, i32>(43)? != 0,
        mixed_language: row.get::<_, i32>(44)? != 0,
        protected_term_hit: row.get::<_, i32>(45)? != 0,
        status: row.get(46)?,
        created_at_ms: row.get(47)?,
        updated_at_ms: row.get(48)?,
    })
}

pub fn should_upload(pool: &DbPool, user_id: &str) -> bool {
    let conn = match pool.get() {
        Ok(c) => c,
        Err(_) => return false,
    };
    let ready: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM telemetry_run_summaries WHERE user_id = ?1 AND status = 'ready'",
            params![user_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let since: i64 = conn
        .query_row(
            "SELECT completed_since_upload FROM telemetry_upload_state WHERE user_id = ?1",
            params![user_id],
            |r| r.get(0),
        )
        .unwrap_or(ready);
    ready >= 10 || since >= 10
}
