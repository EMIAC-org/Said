use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{DbPool, now_ms};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceRun {
    pub run_id: String,
    pub user_id: String,
    pub audio_id: Option<String>,
    pub mode: String,
    pub target_app: Option<String>,
    pub status: String,
    pub wav_bytes: i64,
    pub duration_ms: i64,
    pub pre_transcript: Option<String>,
    pub recording_id: Option<String>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub retryable: bool,
    pub owned_by_airnote: bool,
    pub attempt_count: i64,
    pub completed_successfully: bool,
    pub paste_success: Option<bool>,
    pub diagnostic_json: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub failed_at_ms: Option<i64>,
    pub completed_at_ms: Option<i64>,
}

pub struct CapturedVoiceRun<'a> {
    pub run_id: &'a str,
    pub user_id: &'a str,
    pub audio_id: Option<&'a str>,
    pub mode: &'a str,
    pub target_app: Option<&'a str>,
    pub wav_bytes: i64,
    pub duration_ms: i64,
    pub pre_transcript: Option<&'a str>,
}

pub fn create_voice_run_captured(pool: &DbPool, run: CapturedVoiceRun<'_>) -> Option<()> {
    let conn = pool.get().ok()?;
    let now = now_ms();
    conn.execute(
        "INSERT INTO voice_runs
         (run_id, user_id, audio_id, mode, target_app, status, wav_bytes, duration_ms,
          pre_transcript, retryable, owned_by_airnote, attempt_count,
          completed_successfully, created_at_ms, updated_at_ms)
         VALUES (?1,?2,?3,?4,?5,'captured',?6,?7,?8,0,0,1,0,?9,?9)
         ON CONFLICT(run_id) DO UPDATE SET
            audio_id=excluded.audio_id,
            mode=excluded.mode,
            target_app=excluded.target_app,
            status='captured',
            wav_bytes=excluded.wav_bytes,
            duration_ms=excluded.duration_ms,
            pre_transcript=excluded.pre_transcript,
            error_code=NULL,
            error_message=NULL,
            retryable=0,
            owned_by_airnote=0,
            completed_successfully=0,
            paste_success=NULL,
            diagnostic_json=NULL,
            failed_at_ms=NULL,
            completed_at_ms=NULL,
            updated_at_ms=excluded.updated_at_ms",
        params![
            run.run_id,
            run.user_id,
            run.audio_id,
            run.mode,
            run.target_app,
            run.wav_bytes,
            run.duration_ms,
            run.pre_transcript,
            now,
        ],
    )
    .ok()?;
    Some(())
}

pub fn mark_voice_run_processing(pool: &DbPool, run_id: &str) -> Option<i64> {
    let conn = pool.get().ok()?;
    let now = now_ms();
    let attempts = conn
        .query_row(
            "SELECT COALESCE(attempt_count, 0) + 1 FROM voice_runs WHERE run_id = ?1",
            params![run_id],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(1)
        .max(1);
    conn.execute(
        "UPDATE voice_runs
            SET status='processing',
                attempt_count=?2,
                updated_at_ms=?3
          WHERE run_id=?1",
        params![run_id, attempts, now],
    )
    .ok()
    .filter(|n| *n > 0)?;
    Some(attempts)
}

pub fn mark_voice_run_completed(
    pool: &DbPool,
    run_id: &str,
    recording_id: &str,
    paste_success: Option<bool>,
) -> Option<()> {
    let conn = pool.get().ok()?;
    let now = now_ms();
    conn.execute(
        "UPDATE voice_runs
            SET status='completed',
                recording_id=?2,
                error_code=NULL,
                error_message=NULL,
                retryable=0,
                owned_by_airnote=0,
                completed_successfully=1,
                paste_success=?3,
                completed_at_ms=?4,
                updated_at_ms=?4
          WHERE run_id=?1",
        params![run_id, recording_id, paste_success.map(i64::from), now],
    )
    .ok()
    .filter(|n| *n > 0)?;
    Some(())
}

pub fn mark_voice_run_completed_unlinked(pool: &DbPool, run_id: &str) -> Option<()> {
    let conn = pool.get().ok()?;
    let now = now_ms();
    conn.execute(
        "UPDATE voice_runs
            SET status='completed',
                error_code=NULL,
                error_message=NULL,
                retryable=0,
                owned_by_airnote=0,
                completed_successfully=1,
                completed_at_ms=?2,
                updated_at_ms=?2
          WHERE run_id=?1",
        params![run_id, now],
    )
    .ok()
    .filter(|n| *n > 0)?;
    Some(())
}

pub fn mark_voice_run_failed(
    pool: &DbPool,
    run_id: &str,
    error_code: &str,
    error_message: &str,
    retryable: bool,
    owned_by_airnote: bool,
    diagnostic_json: Option<&Value>,
) -> Option<()> {
    let conn = pool.get().ok()?;
    let now = now_ms();
    let diagnostic = diagnostic_json.map(Value::to_string);
    conn.execute(
        "UPDATE voice_runs
            SET status='failed',
                error_code=?2,
                error_message=?3,
                retryable=?4,
                owned_by_airnote=?5,
                diagnostic_json=?6,
                completed_successfully=0,
                paste_success=NULL,
                failed_at_ms=?7,
                updated_at_ms=?7
          WHERE run_id=?1",
        params![
            run_id,
            error_code,
            error_message,
            i64::from(retryable),
            i64::from(owned_by_airnote),
            diagnostic,
            now,
        ],
    )
    .ok()
    .filter(|n| *n > 0)?;
    Some(())
}

pub fn mark_voice_run_paste_success(
    pool: &DbPool,
    recording_id: &str,
    paste_success: bool,
) -> Option<()> {
    let conn = pool.get().ok()?;
    let now = now_ms();
    conn.execute(
        "UPDATE voice_runs
            SET paste_success=?2,
                updated_at_ms=?3
          WHERE recording_id=?1",
        params![recording_id, i64::from(paste_success), now],
    )
    .ok()
    .filter(|n| *n > 0)?;
    Some(())
}

pub fn mark_voice_run_paste_success_by_run(
    pool: &DbPool,
    run_id: &str,
    paste_success: bool,
) -> Option<()> {
    let conn = pool.get().ok()?;
    let now = now_ms();
    conn.execute(
        "UPDATE voice_runs
            SET paste_success=?2,
                updated_at_ms=?3
          WHERE run_id=?1",
        params![run_id, i64::from(paste_success), now],
    )
    .ok()
    .filter(|n| *n > 0)?;
    Some(())
}

pub fn latest_retryable_failed_voice_run(pool: &DbPool, user_id: &str) -> Option<VoiceRun> {
    let conn = pool.get().ok()?;
    conn.query_row(
        &format!(
            "SELECT {SELECT_COLS}
               FROM voice_runs
              WHERE user_id=?1
                AND status='failed'
                AND retryable=1
                AND audio_id IS NOT NULL
                AND audio_id != ''
              ORDER BY updated_at_ms DESC
              LIMIT 1"
        ),
        params![user_id],
        row_to_voice_run,
    )
    .optional()
    .ok()
    .flatten()
}

pub fn retryable_failed_audio_ids(pool: &DbPool, cutoff_ms: i64) -> Vec<String> {
    let conn = match pool.get() {
        Ok(conn) => conn,
        Err(_) => return Vec::new(),
    };
    let mut stmt = match conn.prepare(
        "SELECT DISTINCT audio_id
           FROM voice_runs
          WHERE status='failed'
            AND retryable=1
            AND audio_id IS NOT NULL
            AND audio_id != ''
            AND updated_at_ms >= ?1",
    ) {
        Ok(stmt) => stmt,
        Err(_) => return Vec::new(),
    };
    stmt.query_map(params![cutoff_ms], |row| row.get::<_, String>(0))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

const SELECT_COLS: &str = "run_id, user_id, audio_id, mode, target_app, status,
    wav_bytes, duration_ms, pre_transcript, recording_id, error_code, error_message,
    retryable, owned_by_airnote, attempt_count, completed_successfully, paste_success,
    diagnostic_json, created_at_ms, updated_at_ms, failed_at_ms, completed_at_ms";

fn row_to_voice_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<VoiceRun> {
    let retryable: i64 = row.get(12)?;
    let owned_by_airnote: i64 = row.get(13)?;
    let completed_successfully: i64 = row.get(15)?;
    let paste_success_raw: Option<i64> = row.get(16)?;
    Ok(VoiceRun {
        run_id: row.get(0)?,
        user_id: row.get(1)?,
        audio_id: row.get(2)?,
        mode: row.get(3)?,
        target_app: row.get(4)?,
        status: row.get(5)?,
        wav_bytes: row.get(6)?,
        duration_ms: row.get(7)?,
        pre_transcript: row.get(8)?,
        recording_id: row.get(9)?,
        error_code: row.get(10)?,
        error_message: row.get(11)?,
        retryable: retryable != 0,
        owned_by_airnote: owned_by_airnote != 0,
        attempt_count: row.get(14)?,
        completed_successfully: completed_successfully != 0,
        paste_success: paste_success_raw.map(|v| v != 0),
        diagnostic_json: row.get(17)?,
        created_at_ms: row.get(18)?,
        updated_at_ms: row.get(19)?,
        failed_at_ms: row.get(20)?,
        completed_at_ms: row.get(21)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store;

    fn pool() -> (DbPool, String) {
        let pool = store::open(&std::env::temp_dir().join(format!(
            "airnote-voice-runs-test-{}.sqlite",
            uuid::Uuid::new_v4()
        )));
        let user_id = store::ensure_default_user(&pool);
        (pool, user_id)
    }

    #[test]
    fn failed_run_with_audio_is_latest_retryable() {
        let (pool, user_id) = pool();
        create_voice_run_captured(
            &pool,
            CapturedVoiceRun {
                run_id: "run-1",
                user_id: &user_id,
                audio_id: Some("audio-1"),
                mode: "normal",
                target_app: Some("com.test"),
                wav_bytes: 100,
                duration_ms: 500,
                pre_transcript: Some("hello"),
            },
        )
        .unwrap();
        mark_voice_run_failed(
            &pool,
            "run-1",
            "sse_missing_done",
            "SSE stream ended without done",
            true,
            true,
            None,
        )
        .unwrap();
        let latest = latest_retryable_failed_voice_run(&pool, &user_id).unwrap();
        assert_eq!(latest.run_id, "run-1");
        assert_eq!(latest.audio_id.as_deref(), Some("audio-1"));
        assert!(latest.retryable);
    }

    #[test]
    fn completed_run_links_recording_and_stops_retry() {
        let (pool, user_id) = pool();
        crate::store::history::insert_recording(
            &pool,
            crate::store::history::InsertRecording {
                id: "rec-2",
                user_id: &user_id,
                transcript: "hello",
                polished: "hello",
                word_count: 1,
                recording_seconds: 1.0,
                model_used: "test",
                confidence: None,
                transcribe_ms: None,
                embed_ms: None,
                polish_ms: None,
                target_app: None,
                source: "voice",
                audio_id: Some("audio-2"),
                enriched_transcript: None,
                raw_transcript: Some("hello"),
                local_corrected_transcript: None,
                polished_output: Some("hello"),
            },
        )
        .unwrap();
        create_voice_run_captured(
            &pool,
            CapturedVoiceRun {
                run_id: "run-2",
                user_id: &user_id,
                audio_id: Some("audio-2"),
                mode: "message_polish",
                target_app: None,
                wav_bytes: 100,
                duration_ms: 500,
                pre_transcript: None,
            },
        )
        .unwrap();
        mark_voice_run_completed(&pool, "run-2", "rec-2", Some(true)).unwrap();
        assert!(latest_retryable_failed_voice_run(&pool, &user_id).is_none());
    }

    #[test]
    fn retryable_failed_audio_ids_respects_cutoff() {
        let (pool, user_id) = pool();
        create_voice_run_captured(
            &pool,
            CapturedVoiceRun {
                run_id: "run-3",
                user_id: &user_id,
                audio_id: Some("audio-3"),
                mode: "normal",
                target_app: None,
                wav_bytes: 100,
                duration_ms: 500,
                pre_transcript: None,
            },
        )
        .unwrap();
        mark_voice_run_failed(
            &pool,
            "run-3",
            "runtime_timeout",
            "timeout",
            true,
            true,
            None,
        )
        .unwrap();
        let ids = retryable_failed_audio_ids(&pool, now_ms() - 1_000);
        assert_eq!(ids, vec!["audio-3".to_string()]);
    }
}
