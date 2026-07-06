use rusqlite::params;
use serde::{Deserialize, Serialize};
use tracing::info;

use super::{DbPool, now_ms};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recording {
    pub id: String,
    pub user_id: String,
    pub timestamp_ms: i64,
    pub transcript: String,
    pub polished: String,
    pub final_text: Option<String>,
    pub word_count: i64,
    pub recording_seconds: f64,
    pub model_used: String,
    pub confidence: Option<f64>,
    pub transcribe_ms: Option<i64>,
    pub embed_ms: Option<i64>,
    pub polish_ms: Option<i64>,
    pub target_app: Option<String>,
    pub edit_count: i64,
    pub source: String,
    pub audio_id: Option<String>,
    pub enriched_transcript: Option<String>,
    pub raw_transcript: Option<String>,
    pub local_corrected_transcript: Option<String>,
    pub polished_output: Option<String>,
    pub trace_json: Option<String>,
}

pub struct InsertRecording<'a> {
    pub id: &'a str,
    pub user_id: &'a str,
    pub transcript: &'a str,
    pub polished: &'a str,
    pub word_count: i64,
    pub recording_seconds: f64,
    pub model_used: &'a str,
    pub confidence: Option<f64>,
    pub transcribe_ms: Option<i64>,
    pub embed_ms: Option<i64>,
    pub polish_ms: Option<i64>,
    pub target_app: Option<&'a str>,
    pub source: &'a str,
    pub audio_id: Option<&'a str>,
    pub enriched_transcript: Option<&'a str>,
    pub raw_transcript: Option<&'a str>,
    pub local_corrected_transcript: Option<&'a str>,
    pub polished_output: Option<&'a str>,
    pub trace_json: Option<&'a str>,
}

pub fn insert_recording(pool: &DbPool, rec: InsertRecording<'_>) -> Option<()> {
    let conn = pool.get().ok()?;
    conn.execute(
        "INSERT INTO recordings
         (id, user_id, timestamp_ms, transcript, polished, word_count, recording_seconds,
          model_used, confidence, transcribe_ms, embed_ms, polish_ms, target_app, source, audio_id,
          enriched_transcript, raw_transcript, local_corrected_transcript, polished_output, trace_json)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20)",
        params![
            rec.id,
            rec.user_id,
            now_ms(),
            rec.transcript,
            rec.polished,
            rec.word_count,
            rec.recording_seconds,
            rec.model_used,
            rec.confidence,
            rec.transcribe_ms,
            rec.embed_ms,
            rec.polish_ms,
            rec.target_app,
            rec.source,
            rec.audio_id,
            rec.enriched_transcript,
            rec.raw_transcript,
            rec.local_corrected_transcript,
            rec.polished_output,
            rec.trace_json,
        ],
    )
    .ok()?;
    Some(())
}

fn row_to_recording(row: &rusqlite::Row<'_>) -> rusqlite::Result<Recording> {
    Ok(Recording {
        id: row.get(0)?,
        user_id: row.get(1)?,
        timestamp_ms: row.get(2)?,
        transcript: row.get(3)?,
        polished: row.get(4)?,
        final_text: row.get(5)?,
        word_count: row.get(6)?,
        recording_seconds: row.get(7)?,
        model_used: row.get(8)?,
        confidence: row.get(9)?,
        transcribe_ms: row.get(10)?,
        embed_ms: row.get(11)?,
        polish_ms: row.get(12)?,
        target_app: row.get(13)?,
        edit_count: row.get(14)?,
        source: row.get(15)?,
        audio_id: row.get(16)?,
        enriched_transcript: row.get(17)?,
        raw_transcript: row.get(18)?,
        local_corrected_transcript: row.get(19)?,
        polished_output: row.get(20)?,
        trace_json: row.get(21)?,
    })
}

const SELECT_COLS: &str = "id, user_id, timestamp_ms, transcript, polished, final_text,
     word_count, recording_seconds, model_used, confidence,
     transcribe_ms, embed_ms, polish_ms, target_app, edit_count, source, audio_id,
     enriched_transcript, raw_transcript, local_corrected_transcript, polished_output, trace_json";

pub fn list_recordings(
    pool: &DbPool,
    user_id: &str,
    limit: i64,
    before_ms: Option<i64>,
) -> Vec<Recording> {
    let conn = match pool.get() {
        Ok(c) => c,
        Err(_) => return vec![],
    };
    let cutoff = before_ms.unwrap_or(i64::MAX);
    let sql = format!(
        "SELECT {SELECT_COLS} FROM recordings
          WHERE user_id = ?1 AND timestamp_ms < ?2
          ORDER BY timestamp_ms DESC LIMIT ?3"
    );
    let mut stmt = match conn.prepare(&sql) {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    stmt.query_map(params![user_id, cutoff, limit], row_to_recording)
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

/// Per-app dictation usage for the Insights "apps you dictate in" section.
/// Groups the local `recordings` table by `target_app`, skipping rows where no
/// app was captured (older dictations, before app-capture shipped), most-used
/// first. App identity (name/category/icon) is resolved on the desktop side from
/// the returned `app` key.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AppUsage {
    /// The `target_app` key (bundle-id on macOS / exe path on Windows).
    pub app: String,
    pub count: i64,
    pub total_words: i64,
    pub last_used_ms: i64,
}

pub fn list_app_usage(pool: &DbPool, user_id: &str) -> Vec<AppUsage> {
    let conn = match pool.get() {
        Ok(c) => c,
        Err(_) => return vec![],
    };
    let sql = "SELECT target_app, COUNT(*), COALESCE(SUM(word_count), 0), MAX(timestamp_ms)
                 FROM recordings
                WHERE user_id = ?1 AND target_app IS NOT NULL AND TRIM(target_app) <> ''
                GROUP BY target_app
                ORDER BY COUNT(*) DESC";
    let mut stmt = match conn.prepare(sql) {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    stmt.query_map(params![user_id], |row| {
        Ok(AppUsage {
            app: row.get(0)?,
            count: row.get(1)?,
            total_words: row.get(2)?,
            last_used_ms: row.get(3)?,
        })
    })
    .map(|rows| rows.flatten().collect())
    .unwrap_or_default()
}

/// Record one browser dictation's site (host). On-device only — this table is
/// never synced to the cloud. Best-effort; a failure just drops the datapoint.
pub fn record_site_visit(pool: &DbPool, user_id: &str, target_app: &str, host: &str) {
    let conn = match pool.get() {
        Ok(c) => c,
        Err(_) => return,
    };
    let _ = conn.execute(
        "INSERT INTO site_visits (user_id, target_app, host, timestamp_ms)
         VALUES (?1, ?2, ?3, ?4)",
        params![user_id, target_app, host, now_ms()],
    );
}

/// Per-site dictation usage for the Insights "Sites you dictate in" section,
/// grouped by host (most-used first). `target_app` is the most-recent browser
/// the host was seen in — for nesting/labelling.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SiteUsage {
    pub host: String,
    pub target_app: String,
    pub count: i64,
    pub last_used_ms: i64,
}

pub fn list_site_usage(pool: &DbPool, user_id: &str) -> Vec<SiteUsage> {
    let conn = match pool.get() {
        Ok(c) => c,
        Err(_) => return vec![],
    };
    let sql = "SELECT host, COUNT(*), MAX(timestamp_ms),
                      (SELECT target_app FROM site_visits s2
                        WHERE s2.user_id = s.user_id AND s2.host = s.host
                        ORDER BY timestamp_ms DESC LIMIT 1)
                 FROM site_visits s
                WHERE user_id = ?1
                GROUP BY host
                ORDER BY COUNT(*) DESC";
    let mut stmt = match conn.prepare(sql) {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    stmt.query_map(params![user_id], |row| {
        Ok(SiteUsage {
            host: row.get(0)?,
            count: row.get(1)?,
            last_used_ms: row.get(2)?,
            target_app: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
        })
    })
    .map(|rows| rows.flatten().collect())
    .unwrap_or_default()
}

/// Delete recordings older than 1 day.
///
/// Best-effort retention only. Driven by the 6 h sweep in `main.rs`, whose
/// interval resets on every backend restart — so in normal per-session use
/// (backend up for minutes, not hours) this rarely fires and recordings
/// effectively persist. Do not rely on it as a guaranteed retention bound;
/// anything that needs durable history should live in its own table.
pub fn cleanup_old_recordings(pool: &DbPool) {
    let conn = match pool.get() {
        Ok(c) => c,
        Err(_) => return,
    };
    let one_day_ms = 86_400_000i64;
    let cutoff = now_ms() - one_day_ms;
    match conn.execute(
        "DELETE FROM recordings WHERE timestamp_ms < ?1",
        params![cutoff],
    ) {
        Ok(n) if n > 0 => info!("cleaned up {n} old recordings (>1 day)"),
        _ => {}
    }
}

pub fn get_recording(pool: &DbPool, id: &str) -> Option<Recording> {
    let conn = pool.get().ok()?;
    let sql = format!("SELECT {SELECT_COLS} FROM recordings WHERE id = ?1");
    conn.query_row(&sql, params![id], row_to_recording).ok()
}

/// Hard-delete a single recording by id. Returns true if a row was deleted.
pub fn delete_recording(pool: &DbPool, id: &str) -> bool {
    let conn = match pool.get() {
        Ok(c) => c,
        Err(_) => return false,
    };
    conn.execute("DELETE FROM recordings WHERE id = ?1", params![id])
        .map(|n| n > 0)
        .unwrap_or(false)
}

pub fn set_recording_audio_id(pool: &DbPool, id: &str, audio_id: &str) -> Option<()> {
    let conn = pool.get().ok()?;
    conn.execute(
        "UPDATE recordings SET audio_id = ?1 WHERE id = ?2",
        params![audio_id, id],
    )
    .ok()
    .filter(|n| *n > 0)?;
    Some(())
}

pub fn merge_recording_trace(
    pool: &DbPool,
    id: &str,
    trace: &serde_json::Value,
) -> Result<(), String> {
    let conn = pool.get().map_err(|e| e.to_string())?;
    let existing: Option<String> = conn
        .query_row(
            "SELECT trace_json FROM recordings WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .ok();
    let existing_value = existing
        .as_deref()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok());
    let merged =
        said_core::dictation_trace::merge_trace_values(existing_value.as_ref(), Some(trace));
    conn.execute(
        "UPDATE recordings SET trace_json = ?1 WHERE id = ?2",
        params![merged.to_string(), id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn apply_edit_feedback(pool: &DbPool, recording_id: &str, user_kept: &str) -> Option<()> {
    let conn = pool.get().ok()?;
    conn.execute(
        "UPDATE recordings
            SET final_text = ?1,
                edit_count = CASE
                    WHEN final_text IS NULL OR final_text != ?1 THEN edit_count + 1
                    ELSE edit_count
                END
          WHERE id = ?2",
        params![user_kept, recording_id],
    )
    .ok()?;
    Some(())
}
