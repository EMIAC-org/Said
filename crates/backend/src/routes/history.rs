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
    let items = crate::store::history::list_recordings(&state.pool, &user_id, q.limit, q.before);
    Ok(Json(items))
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
