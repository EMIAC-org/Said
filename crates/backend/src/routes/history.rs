use axum::{
    Json,
    body::Body,
    extract::{Multipart, Path, Query, State},
    http::{StatusCode, header},
    response::Response,
};
use serde::Deserialize;
use tokio::fs::File;
use tokio_util::io::ReaderStream;
use tracing::warn;
use uuid::Uuid;

use crate::{AppState, store::history::Recording};

#[derive(Deserialize)]
pub struct HistoryQuery {
    #[serde(default = "default_limit")]
    limit: i64,
    before: Option<i64>,
}

fn default_limit() -> i64 {
    50
}

pub async fn list(
    State(state): State<AppState>,
    Query(q): Query<HistoryQuery>,
) -> Result<Json<Vec<Recording>>, StatusCode> {
    let user_id = state.default_user_id.clone();
    let mut items = match try_list_server_history(&state, &user_id, &q).await {
        Some(items) => items,
        None => crate::store::history::list_recordings(&state.pool, &user_id, q.limit, q.before),
    };
    enrich_recordings_with_local_audio(&state.pool, &user_id, &mut items);
    Ok(Json(items))
}

#[derive(Debug, Deserialize)]
struct RuntimeHistoryItem {
    id: Uuid,
    recording_id: Option<String>,
    source: String,
    raw_transcript: Option<String>,
    transcript: Option<String>,
    local_corrected_transcript: Option<String>,
    polished_output: Option<String>,
    final_text: Option<String>,
    model_used: Option<String>,
    word_count: Option<i64>,
    recording_seconds: Option<f64>,
    transcribe_ms: Option<i64>,
    embed_ms: Option<i64>,
    polish_ms: Option<i64>,
    target_app: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
}

async fn try_list_server_history(
    state: &AppState,
    user_id: &str,
    q: &HistoryQuery,
) -> Option<Vec<Recording>> {
    let user = crate::store::users::get_user(&state.pool, user_id)?;
    let token = user.cloud_token.filter(|s| !s.trim().is_empty())?;
    let base_url = user
        .enterprise_server_url
        .filter(|s| !s.trim().is_empty())?;
    let url = format!("{}/v1/runtime/history", base_url.trim_end_matches('/'));

    let mut query = vec![("limit", q.limit.to_string())];
    if let Some(before_ms) = q.before {
        if let Some(before) = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(before_ms)
            .map(|ts| ts.to_rfc3339())
        {
            query.push(("before", before));
        }
    }

    let resp = match state
        .http_client
        .get(&url)
        .bearer_auth(token)
        .query(&query)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(e) => {
            warn!("[history] server history unavailable, using local history: {e}");
            return None;
        }
    };

    let status = resp.status();
    if !status.is_success() {
        warn!("[history] server history returned {status}, using local history");
        return None;
    }

    match resp.json::<Vec<RuntimeHistoryItem>>().await {
        Ok(rows) => Some(
            rows.into_iter()
                .map(|row| server_row_to_recording(row, user_id))
                .collect(),
        ),
        Err(e) => {
            warn!("[history] server history decode failed, using local history: {e}");
            None
        }
    }
}

fn server_row_to_recording(row: RuntimeHistoryItem, user_id: &str) -> Recording {
    let transcript = first_non_empty([
        row.transcript.as_deref(),
        row.local_corrected_transcript.as_deref(),
        row.raw_transcript.as_deref(),
    ])
    .unwrap_or_default();
    // History stores strictly what AirNote OUTPUT (the polished paste), never the
    // user's later manual correction. `final_text` (the 30s edit-watch capture)
    // feeds the learning pipeline only — it must not become the displayed heading.
    let polished = first_non_empty([
        row.polished_output.as_deref(),
        row.final_text.as_deref(),
        Some(transcript.as_str()),
    ])
    .unwrap_or_default();
    let word_count = row.word_count.unwrap_or_else(|| count_words(&polished));

    Recording {
        id: row
            .recording_id
            .filter(|id| !id.trim().is_empty())
            .unwrap_or_else(|| row.id.to_string()),
        user_id: user_id.to_string(),
        timestamp_ms: row.created_at.timestamp_millis(),
        transcript,
        polished,
        final_text: row.final_text,
        word_count,
        recording_seconds: row.recording_seconds.unwrap_or(0.0),
        model_used: row
            .model_used
            .unwrap_or_else(|| "server_runtime".to_string()),
        confidence: None,
        transcribe_ms: row.transcribe_ms,
        embed_ms: row.embed_ms,
        polish_ms: row.polish_ms,
        target_app: row.target_app,
        edit_count: 0,
        source: row.source,
        audio_id: None,
        enriched_transcript: None,
        raw_transcript: row.raw_transcript,
        local_corrected_transcript: row.local_corrected_transcript,
        polished_output: row.polished_output,
        trace_json: None,
    }
}

fn first_non_empty<'a>(values: impl IntoIterator<Item = Option<&'a str>>) -> Option<String> {
    values
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|s| !s.is_empty())
        .map(str::to_string)
}

fn count_words(text: &str) -> i64 {
    text.split_whitespace().count() as i64
}

/// Server runtime history omits audio_id; merge from local SQLite when the row
/// still exists on this device so play/download work in the History UI.
fn enrich_recordings_with_local_audio(
    pool: &crate::store::DbPool,
    user_id: &str,
    recordings: &mut [Recording],
) {
    use std::collections::HashMap;

    let local_rows: Vec<Recording> =
        crate::store::history::list_recordings(pool, user_id, 500, None);
    let mut local_by_ts: HashMap<i64, Recording> = HashMap::new();
    let mut local_by_id: HashMap<String, Recording> = HashMap::new();
    let mut local_with_audio: Vec<Recording> = Vec::new();
    for local in local_rows {
        if local.audio_id.is_some() {
            local_by_id
                .entry(local.id.clone())
                .or_insert_with(|| local.clone());
            local_by_ts
                .entry(local.timestamp_ms)
                .or_insert_with(|| local.clone());
            local_with_audio.push(local);
        }
    }

    for rec in recordings.iter_mut() {
        if rec.audio_id.is_some() {
            continue;
        }
        if let Some(local) = local_by_id.get(&rec.id) {
            rec.audio_id = local.audio_id.clone();
            continue;
        }
        // Server row id may differ from the local SQLite id — match by timestamp.
        if let Some(local) = local_by_ts.get(&rec.timestamp_ms) {
            rec.id = local.id.clone();
            rec.audio_id = local.audio_id.clone();
            continue;
        }
        if let Some(local) = find_best_local_audio_match(rec, &local_with_audio) {
            rec.id = local.id.clone();
            rec.audio_id = local.audio_id.clone();
        }
    }
}

fn find_best_local_audio_match<'a>(
    rec: &Recording,
    local_rows: &'a [Recording],
) -> Option<&'a Recording> {
    const EXACTISH_WINDOW_MS: i64 = 30_000;
    const CONTENT_WINDOW_MS: i64 = 10 * 60_000;

    local_rows
        .iter()
        .filter(|local| {
            let delta = (local.timestamp_ms - rec.timestamp_ms).abs();
            if delta <= EXACTISH_WINDOW_MS {
                return true;
            }

            let polished_match =
                !rec.polished.trim().is_empty() && rec.polished.trim() == local.polished.trim();
            let transcript_match = !rec.transcript.trim().is_empty()
                && rec.transcript.trim() == local.transcript.trim();

            delta <= CONTENT_WINDOW_MS && (polished_match || transcript_match)
        })
        .min_by_key(|local| {
            let delta = (local.timestamp_ms - rec.timestamp_ms).abs();
            let polished_mismatch = if rec.polished.trim() == local.polished.trim() {
                0
            } else {
                1
            };
            let transcript_mismatch = if rec.transcript.trim() == local.transcript.trim() {
                0
            } else {
                1
            };
            (polished_mismatch, transcript_mismatch, delta)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime_row() -> RuntimeHistoryItem {
        RuntimeHistoryItem {
            id: Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap(),
            recording_id: Some("local-rec-1".to_string()),
            source: "voice".to_string(),
            raw_transcript: Some("raw words".to_string()),
            transcript: Some("corrected words".to_string()),
            local_corrected_transcript: Some("local corrected words".to_string()),
            polished_output: Some("polished words".to_string()),
            final_text: Some("final words kept".to_string()),
            model_used: Some("server-model".to_string()),
            word_count: Some(3),
            recording_seconds: Some(2.5),
            transcribe_ms: Some(100),
            embed_ms: Some(20),
            polish_ms: Some(300),
            target_app: Some("Notes".to_string()),
            created_at: chrono::DateTime::parse_from_rfc3339("2026-06-08T12:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
        }
    }

    #[test]
    fn server_row_maps_to_local_recording_shape() {
        let rec = server_row_to_recording(runtime_row(), "user-1");

        assert_eq!(rec.id, "local-rec-1");
        assert_eq!(rec.user_id, "user-1");
        assert_eq!(rec.timestamp_ms, 1780920000000);
        assert_eq!(rec.transcript, "corrected words");
        // History shows AirNote's original output (`polished_output`), NOT the
        // user's post-paste edit (`final_text`) — even though final_text is set.
        assert_eq!(rec.polished, "polished words");
        assert_eq!(rec.final_text.as_deref(), Some("final words kept"));
        assert_eq!(rec.word_count, 3);
        assert_eq!(rec.recording_seconds, 2.5);
        assert_eq!(rec.model_used, "server-model");
        assert_eq!(rec.transcribe_ms, Some(100));
        assert_eq!(rec.embed_ms, Some(20));
        assert_eq!(rec.polish_ms, Some(300));
        assert_eq!(rec.target_app.as_deref(), Some("Notes"));
        assert_eq!(rec.edit_count, 0);
        assert!(rec.audio_id.is_none());
    }

    #[test]
    fn server_row_falls_back_for_missing_optional_fields() {
        let mut row = runtime_row();
        row.recording_id = None;
        row.transcript = None;
        row.polished_output = None;
        row.final_text = Some("kept final words".to_string());
        row.model_used = None;
        row.word_count = None;
        row.recording_seconds = None;

        let rec = server_row_to_recording(row, "default");

        assert_eq!(rec.id, "11111111-1111-1111-1111-111111111111");
        assert_eq!(rec.transcript, "local corrected words");
        assert_eq!(rec.polished, "kept final words");
        assert_eq!(rec.word_count, 3);
        assert_eq!(rec.recording_seconds, 0.0);
        assert_eq!(rec.model_used, "server_runtime");
    }
}

pub async fn delete(State(state): State<AppState>, Path(id): Path<String>) -> StatusCode {
    // Also delete the WAV file if audio_id is linked
    if let Some(rec) = crate::store::history::get_recording(&state.pool, &id) {
        if let Some(audio_id) = rec.audio_id {
            let wav = audio_dir().join(format!("{audio_id}.wav"));
            let _ = std::fs::remove_file(wav);
        }
    }
    if crate::store::history::delete_recording(&state.pool, &id) {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

pub async fn audio(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response<Body>, StatusCode> {
    let rec =
        crate::store::history::get_recording(&state.pool, &id).ok_or(StatusCode::NOT_FOUND)?;

    let audio_id = rec.audio_id.ok_or(StatusCode::NOT_FOUND)?;
    let path = audio_dir().join(format!("{audio_id}.wav"));

    let file = File::open(&path).await.map_err(|_| StatusCode::NOT_FOUND)?;
    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    Ok(Response::builder()
        .status(200)
        .header(header::CONTENT_TYPE, "audio/wav")
        .header(header::CACHE_CONTROL, "no-store")
        .body(body)
        .unwrap())
}

pub async fn upload_audio(
    State(state): State<AppState>,
    Path(id): Path<String>,
    mut multipart: Multipart,
) -> StatusCode {
    if crate::store::history::get_recording(&state.pool, &id).is_none() {
        return StatusCode::NOT_FOUND;
    }

    let mut wav_data = Vec::new();
    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name() == Some("audio") {
            wav_data = field.bytes().await.unwrap_or_default().to_vec();
            break;
        }
    }

    if wav_data.is_empty() {
        return StatusCode::BAD_REQUEST;
    }

    let audio_id = Uuid::new_v4().to_string();
    let dir = audio_dir();
    let path = dir.join(format!("{audio_id}.wav"));
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        warn!("[history] failed to create audio dir: {e}");
        return StatusCode::INTERNAL_SERVER_ERROR;
    }
    if let Err(e) = tokio::fs::write(&path, wav_data).await {
        warn!("[history] failed to save uploaded audio: {e}");
        return StatusCode::INTERNAL_SERVER_ERROR;
    }

    let pool = state.pool.clone();
    let recording_id = id.clone();
    let audio_id_for_db = audio_id.clone();
    let linked = tokio::task::spawn_blocking(move || {
        crate::store::history::set_recording_audio_id(&pool, &recording_id, &audio_id_for_db)
            .is_some()
    })
    .await
    .unwrap_or(false);

    if linked {
        StatusCode::NO_CONTENT
    } else {
        let _ = tokio::fs::remove_file(path).await;
        StatusCode::NOT_FOUND
    }
}

fn audio_dir() -> std::path::PathBuf {
    let base = dirs::data_local_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join(".local/share")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("VoicePolish").join("audio")
}
