//! The voice pipeline — the heart of the server-side migration.
//!
//!   WS   /v1/runtime/voice?session_id&session_token   — streaming dictation
//!   POST /v1/runtime/voice/batch  (and /v1/mobile/dictate) — batch fallback
//!
//! Streaming flow: client sends 16 kHz PCM16 binary frames; the gateway relays
//! them to Deepgram (nova-3), emits `stt.interim`/`stt.final`, then on stop runs
//! Groq polish (streaming `polish.delta`), applies the Hinglish script guard,
//! and emits a single insertable `final`. Run metrics + provider cost are
//! persisted off the hot path. No raw audio or transcript text is stored.
//!
//! When provider keys are absent the pipeline runs a deterministic MOCK so the
//! iOS app can be exercised end-to-end against staging without keys.

use std::time::Instant;

use axum::{
    Json,
    extract::{
        Multipart, Query, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::StatusCode,
    response::IntoResponse,
};
use chrono::{DateTime, Utc};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as DgMessage;
use tracing::warn;
use uuid::Uuid;

use crate::{AppState, auth::AuthUser, runtime, util::*};

#[derive(Debug, Deserialize)]
pub struct VoiceWsQuery {
    pub session_id: Uuid,
    pub session_token: Uuid,
}

pub async fn voice_ws(
    State(state): State<AppState>,
    Query(query): Query<VoiceWsQuery>,
    ws: WebSocketUpgrade,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let row: Option<(Uuid, Uuid, String, String, String, String, DateTime<Utc>)> = sqlx::query_as(
        "SELECT account_id, id, device_id, language_hint, style, current_vocab_hash, expires_at
           FROM voice_sessions
          WHERE id = $1 AND session_token = $2",
    )
    .bind(query.session_id)
    .bind(query.session_token)
    .fetch_optional(&state.db)
    .await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "database error".to_string()))?;

    let Some((account_id, session_id, device_id, language, style, _vocab_hash, expires_at)) = row
    else {
        return Err((StatusCode::UNAUTHORIZED, "invalid runtime session".to_string()));
    };
    if expires_at < Utc::now() {
        return Err((StatusCode::GONE, "runtime session expired".to_string()));
    }

    let vocab_terms = runtime::vocab::load_terms_for_prompt(&state.db, account_id).await;

    let run_id: Uuid = sqlx::query_scalar(
        "INSERT INTO voice_runs (session_id, account_id, device_id, mode, status, language, style)
         VALUES ($1,$2,$3,'stream','stream_open',$4,$5)
         RETURNING id",
    )
    .bind(session_id)
    .bind(account_id)
    .bind(&device_id)
    .bind(&language)
    .bind(&style)
    .fetch_one(&state.db)
    .await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "database error".to_string()))?;

    let _ = sqlx::query("UPDATE voice_sessions SET status = 'stream_open' WHERE id = $1")
        .bind(session_id)
        .execute(&state.db)
        .await;

    let st = state.clone();
    Ok(ws.on_upgrade(move |socket| {
        handle_voice_socket(st, socket, account_id, session_id, run_id, language, style, vocab_terms)
    }))
}

#[allow(clippy::too_many_arguments)]
async fn handle_voice_socket(
    state: AppState,
    socket: WebSocket,
    account_id: Uuid,
    session_id: Uuid,
    run_id: Uuid,
    language: String,
    style: String,
    vocab_terms: Vec<String>,
) {
    let (mut client_sink, mut client_stream) = socket.split();
    let started = Instant::now();

    if client_sink
        .send(Message::Text(
            json!({"type": "session.ready", "session_id": session_id, "run_id": run_id})
                .to_string(),
        ))
        .await
        .is_err()
    {
        return;
    }

    let mock = state.deepgram_api_key.trim().is_empty() || state.llm_api_key.trim().is_empty();

    // ── Phase 1: capture audio → transcript ──────────────────────────────────
    let mut transcript = String::new();
    let mut frames: i32 = 0;
    let mut bytes: i32 = 0;
    let stt_started = Instant::now();

    if mock {
        loop {
            match client_stream.next().await {
                Some(Ok(Message::Binary(b))) => {
                    frames += 1;
                    bytes += b.len() as i32;
                }
                Some(Ok(Message::Text(t))) => {
                    if is_stop(&t) {
                        break;
                    }
                }
                Some(Ok(Message::Close(_))) | None => break,
                Some(Ok(_)) => {}
                Some(Err(_)) => break,
            }
        }
        transcript = mock_transcript(&language);
    } else {
        let dg = match runtime::stt::connect_stream(&state.deepgram_api_key, &language).await {
            Ok(s) => s,
            Err(e) => {
                warn!("[voice] deepgram connect failed: {e}");
                let _ = client_sink
                    .send(Message::Text(
                        json!({"type": "error", "code": "stt_unavailable", "retryable": true,
                               "message": "Voice service is unavailable. Try again or copy from history."})
                        .to_string(),
                    ))
                    .await;
                finalize_failed(&state.db, run_id, Some(session_id), "stt_unavailable", frames, bytes)
                    .await;
                let _ = client_sink.send(Message::Close(None)).await;
                return;
            }
        };
        let (mut dg_sink, mut dg_stream) = dg.split();

        'capture: loop {
            tokio::select! {
                client_msg = client_stream.next() => {
                    match client_msg {
                        Some(Ok(Message::Binary(b))) => {
                            frames += 1;
                            bytes += b.len() as i32;
                            let _ = dg_sink.send(DgMessage::Binary(b.to_vec())).await;
                        }
                        Some(Ok(Message::Text(t))) => {
                            if is_stop(&t) {
                                let _ = dg_sink
                                    .send(DgMessage::Text(json!({"type": "CloseStream"}).to_string()))
                                    .await;
                                break 'capture;
                            } else if is_start(&t) {
                                let _ = client_sink
                                    .send(Message::Text(
                                        json!({"type": "runtime.status", "status": "ready_for_audio"})
                                            .to_string(),
                                    ))
                                    .await;
                            }
                        }
                        Some(Ok(Message::Close(_))) | None => {
                            let _ = dg_sink
                                .send(DgMessage::Text(json!({"type": "CloseStream"}).to_string()))
                                .await;
                            break 'capture;
                        }
                        Some(Ok(_)) => {}
                        Some(Err(_)) => {
                            let _ = dg_sink
                                .send(DgMessage::Text(json!({"type": "CloseStream"}).to_string()))
                                .await;
                            break 'capture;
                        }
                    }
                }
                dg_msg = dg_stream.next() => {
                    match dg_msg {
                        Some(Ok(DgMessage::Text(txt))) => {
                            if let Some((piece, is_final)) = runtime::stt::extract_transcript(&txt) {
                                if is_final {
                                    append_transcript(&mut transcript, &piece);
                                    let _ = client_sink
                                        .send(Message::Text(
                                            json!({"type": "stt.final", "text": piece}).to_string(),
                                        ))
                                        .await;
                                } else {
                                    let _ = client_sink
                                        .send(Message::Text(
                                            json!({"type": "stt.interim", "text": piece}).to_string(),
                                        ))
                                        .await;
                                }
                            }
                        }
                        Some(Ok(DgMessage::Close(_))) | None => break 'capture,
                        Some(Ok(_)) => {}
                        Some(Err(e)) => {
                            warn!("[voice] deepgram read error: {e}");
                            break 'capture;
                        }
                    }
                }
            }
        }

        // Drain any trailing finals Deepgram flushes after CloseStream.
        while let Some(dg_msg) = dg_stream.next().await {
            match dg_msg {
                Ok(DgMessage::Text(txt)) => {
                    if let Some((piece, true)) = runtime::stt::extract_transcript(&txt) {
                        append_transcript(&mut transcript, &piece);
                        let _ = client_sink
                            .send(Message::Text(
                                json!({"type": "stt.final", "text": piece}).to_string(),
                            ))
                            .await;
                    }
                }
                Ok(DgMessage::Close(_)) => break,
                Err(_) => break,
                _ => {}
            }
        }
        let _ = dg_sink.close().await;
    }

    let stt_ms = stt_started.elapsed().as_millis() as i64;
    let transcript = transcript.trim().to_string();

    if transcript.is_empty() {
        let _ = client_sink
            .send(Message::Text(
                json!({"type": "error", "code": "empty_transcript", "retryable": true,
                       "message": "Didn't catch that. Try again."})
                .to_string(),
            ))
            .await;
        finalize_failed(&state.db, run_id, Some(session_id), "empty_transcript", frames, bytes).await;
        let _ = client_sink.send(Message::Close(None)).await;
        return;
    }

    // ── Phase 2: polish → guard → final ──────────────────────────────────────
    let _ = client_sink
        .send(Message::Text(
            json!({"type": "polish.started", "model": state.llm_model}).to_string(),
        ))
        .await;

    let system_prompt = runtime::prompt::build_system_prompt(&language, &style, &vocab_terms);
    let user_message = runtime::prompt::build_user_message(&transcript, &language);

    let mut polish_failed = false;
    let polish_ms: i64;
    let polished_raw: String;

    if mock {
        let mock_out = mock_polish(&transcript);
        for tok in chunk_tokens(&mock_out) {
            let _ = client_sink
                .send(Message::Text(
                    json!({"type": "polish.delta", "token": tok}).to_string(),
                ))
                .await;
        }
        polished_raw = mock_out;
        polish_ms = 0;
    } else {
        let (tx, mut rx) = mpsc::channel::<String>(128);
        let st2 = state.clone();
        let sys = system_prompt.clone();
        let usr = user_message.clone();
        let handle = tokio::spawn(async move {
            runtime::polish::stream_polish(
                &st2.http,
                &st2.llm_api_key,
                &st2.llm_base_url,
                &st2.llm_model,
                &sys,
                &usr,
                tx,
            )
            .await
        });

        while let Some(tok) = rx.recv().await {
            if client_sink
                .send(Message::Text(
                    json!({"type": "polish.delta", "token": tok}).to_string(),
                ))
                .await
                .is_err()
            {
                break;
            }
        }

        match handle.await {
            Ok(Ok(outcome)) => {
                polished_raw = outcome.polished;
                polish_ms = outcome.polish_ms as i64;
            }
            Ok(Err(e)) => {
                warn!("[voice] polish failed: {e}");
                polished_raw = transcript.clone();
                polish_failed = true;
                polish_ms = 0;
            }
            Err(e) => {
                warn!("[voice] polish task join error: {e}");
                polished_raw = transcript.clone();
                polish_failed = true;
                polish_ms = 0;
            }
        }
    }

    if polish_failed {
        let _ = client_sink
            .send(Message::Text(
                json!({"type": "guard.warning", "code": "polish_failed_faithful_transcript"})
                    .to_string(),
            ))
            .await;
    }

    let polished = runtime::script::apply_script_guard(polished_raw.trim(), &language);
    let latency_ms = started.elapsed().as_millis() as i64;

    finalize_success(
        &state.db,
        run_id,
        Some(session_id),
        &language,
        &style,
        transcript.chars().count() as i32,
        polished.chars().count() as i32,
        frames,
        bytes,
        stt_ms,
        polish_ms,
        latency_ms,
    )
    .await;
    record_usage(&state.db, account_id, run_id, bytes, &user_message, &polished).await;

    let _ = client_sink
        .send(Message::Text(
            json!({
                "type": "final",
                "request_id": run_id,
                "session_id": session_id,
                "transcript": transcript,
                "polished": polished,
                "language": language,
                "style": style,
                "latency_ms": latency_ms,
                "mock": mock
            })
            .to_string(),
        ))
        .await;
    let _ = client_sink
        .send(Message::Text(
            json!({"type": "runtime.done", "run_id": run_id, "session_id": session_id}).to_string(),
        ))
        .await;
    let _ = client_sink.send(Message::Close(None)).await;
}

// ── Batch fallback ────────────────────────────────────────────────────────────

pub async fn dictate_batch(
    State(state): State<AppState>,
    user: AuthUser,
    mut multipart: Multipart,
) -> ApiResult<Json<Value>> {
    let mut audio: Vec<u8> = Vec::new();
    let mut audio_content_type = String::new();
    let mut session_id: Option<Uuid> = None;
    let mut device_id = "batch".to_string();
    let mut language = "auto".to_string();
    let mut style = "work".to_string();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|_| bad_request("invalid multipart body"))?
    {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "audio" => {
                audio_content_type = field.content_type().unwrap_or("").to_string();
                audio = field
                    .bytes()
                    .await
                    .map_err(|_| bad_request("audio read failed"))?
                    .to_vec();
            }
            "session_id" => {
                session_id = field
                    .text()
                    .await
                    .ok()
                    .and_then(|s| Uuid::parse_str(s.trim()).ok());
            }
            "device_id" => {
                if let Ok(v) = field.text().await {
                    let v = v.trim();
                    if !v.is_empty() {
                        device_id = v.chars().take(MAX_DEVICE_ID_LEN).collect();
                    }
                }
            }
            "locale_hint" | "language_hint" | "language" => {
                let v = field.text().await.unwrap_or_default();
                language = normalize_choice(Some(&v), LANGUAGES, "auto");
            }
            "style" => {
                let v = field.text().await.unwrap_or_default();
                style = normalize_choice(Some(&v), STYLES, "work");
            }
            // before_text / after_text / selected_text / client_request_id / etc:
            // drain without storing raw text.
            _ => {
                let _ = field.bytes().await;
            }
        }
    }

    if audio.is_empty() {
        return Err(bad_request("audio field required"));
    }

    let started = Instant::now();
    let mock = state.deepgram_api_key.trim().is_empty() || state.llm_api_key.trim().is_empty();
    let audio_bytes = audio.len() as i32;

    let run_id: Uuid = sqlx::query_scalar(
        "INSERT INTO voice_runs (session_id, account_id, device_id, mode, status, language, style)
         VALUES ($1,$2,$3,'batch','created',$4,$5)
         RETURNING id",
    )
    .bind(session_id)
    .bind(user.account_id)
    .bind(&device_id)
    .bind(&language)
    .bind(&style)
    .fetch_one(&state.db)
    .await
    .map_err(db_err)?;

    let stt_started = Instant::now();
    let transcript = if mock {
        mock_transcript(&language)
    } else {
        match runtime::stt::transcribe_batch(
            &state.http,
            &state.deepgram_api_key,
            audio,
            &audio_content_type,
            &language,
        )
        .await
        {
            Ok(t) => t,
            Err(e) => {
                warn!("[voice-batch] stt failed: {e}");
                finalize_failed(&state.db, run_id, session_id, "stt_failed", 0, audio_bytes).await;
                return Err((
                    StatusCode::BAD_GATEWAY,
                    Json(json!({"error": "stt_failed", "message": e, "request_id": run_id})),
                ));
            }
        }
    };
    let stt_ms = stt_started.elapsed().as_millis() as i64;

    let transcript = transcript.trim().to_string();
    if transcript.is_empty() {
        finalize_failed(&state.db, run_id, session_id, "empty_transcript", 0, audio_bytes).await;
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"error": "empty_transcript", "request_id": run_id})),
        ));
    }

    let vocab_terms = runtime::vocab::load_terms_for_prompt(&state.db, user.account_id).await;
    let system_prompt = runtime::prompt::build_system_prompt(&language, &style, &vocab_terms);
    let user_message = runtime::prompt::build_user_message(&transcript, &language);

    let polish_started = Instant::now();
    let polished_raw = if mock {
        mock_polish(&transcript)
    } else {
        match runtime::polish::polish_once(
            &state.http,
            &state.llm_api_key,
            &state.llm_base_url,
            &state.llm_model,
            &system_prompt,
            &user_message,
        )
        .await
        {
            Ok(p) if !p.trim().is_empty() => p,
            // Faithful fallback: insert the transcript if polish fails/empties.
            _ => transcript.clone(),
        }
    };
    let polish_ms = polish_started.elapsed().as_millis() as i64;
    let polished = runtime::script::apply_script_guard(polished_raw.trim(), &language);
    let latency_ms = started.elapsed().as_millis() as i64;

    finalize_success(
        &state.db,
        run_id,
        session_id,
        &language,
        &style,
        transcript.chars().count() as i32,
        polished.chars().count() as i32,
        0,
        audio_bytes,
        stt_ms,
        polish_ms,
        latency_ms,
    )
    .await;
    record_usage(&state.db, user.account_id, run_id, audio_bytes, &user_message, &polished).await;

    Ok(Json(json!({
        "schema": "airnote.runtime.dictate.v1",
        "request_id": run_id,
        "session_id": session_id,
        "transcript": transcript,
        "polished": polished,
        "language": language,
        "style": style,
        "latency_ms": latency_ms,
        "mock": mock
    })))
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn append_transcript(acc: &mut String, piece: &str) {
    if piece.is_empty() {
        return;
    }
    if !acc.is_empty() {
        acc.push(' ');
    }
    acc.push_str(piece);
}

fn control_type(text: &str) -> Option<String> {
    serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|v| v.get("type").and_then(Value::as_str).map(str::to_string))
}

fn is_stop(text: &str) -> bool {
    control_type(text)
        .map(|t| {
            matches!(
                t.as_str(),
                "audio.end" | "audio_end" | "voice.stop" | "session.stop" | "session_stop" | "stop"
            )
        })
        .unwrap_or(false)
}

fn is_start(text: &str) -> bool {
    control_type(text)
        .map(|t| {
            matches!(
                t.as_str(),
                "voice.start" | "session.start" | "session_start" | "auth.hello" | "start"
            )
        })
        .unwrap_or(false)
}

fn mock_transcript(language: &str) -> String {
    match language {
        "en" => "send the concise update to rahul tomorrow".to_string(),
        _ => "kal ka update concise banake rahul ko bhej do".to_string(),
    }
}

/// Deterministic offline "polish": capitalize the first letter and ensure
/// terminal punctuation. Keeps staging output stable without provider keys.
fn mock_polish(transcript: &str) -> String {
    let t = transcript.trim();
    if t.is_empty() {
        return String::new();
    }
    let mut chars = t.chars();
    let first: String = chars.next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
    let rest: String = chars.collect();
    let mut out = format!("{first}{rest}");
    if !out.ends_with(['.', '!', '?']) {
        out.push('.');
    }
    out
}

fn chunk_tokens(s: &str) -> Vec<String> {
    s.split_inclusive(' ').map(str::to_string).collect()
}

async fn finalize_success(
    db: &PgPool,
    run_id: Uuid,
    session_id: Option<Uuid>,
    language: &str,
    style: &str,
    transcript_chars: i32,
    polished_chars: i32,
    frames: i32,
    bytes: i32,
    stt_ms: i64,
    polish_ms: i64,
    latency_ms: i64,
) {
    let _ = sqlx::query(
        "UPDATE voice_runs
            SET status = 'completed',
                language = $2,
                style = $3,
                transcript_char_count = $4,
                polished_char_count = $5,
                audio_frame_count = $6,
                audio_byte_count = $7,
                stt_ms = $8,
                polish_ms = $9,
                latency_ms = $10,
                completed_at = now()
          WHERE id = $1",
    )
    .bind(run_id)
    .bind(language)
    .bind(style)
    .bind(transcript_chars)
    .bind(polished_chars)
    .bind(frames)
    .bind(bytes)
    .bind(stt_ms as i32)
    .bind(polish_ms as i32)
    .bind(latency_ms as i32)
    .execute(db)
    .await;

    if let Some(sid) = session_id {
        let _ = sqlx::query(
            "UPDATE voice_sessions SET status = 'completed', completed_at = now() WHERE id = $1",
        )
        .bind(sid)
        .execute(db)
        .await;
    }
}

async fn finalize_failed(
    db: &PgPool,
    run_id: Uuid,
    session_id: Option<Uuid>,
    code: &str,
    frames: i32,
    bytes: i32,
) {
    let _ = sqlx::query(
        "UPDATE voice_runs
            SET status = 'failed', error_code = $2, audio_frame_count = $3,
                audio_byte_count = $4, completed_at = now()
          WHERE id = $1",
    )
    .bind(run_id)
    .bind(code)
    .bind(frames)
    .bind(bytes)
    .execute(db)
    .await;

    if let Some(sid) = session_id {
        let _ = sqlx::query(
            "UPDATE voice_sessions SET status = 'failed', completed_at = now() WHERE id = $1",
        )
        .bind(sid)
        .execute(db)
        .await;
    }
}

async fn record_usage(
    db: &PgPool,
    account_id: Uuid,
    run_id: Uuid,
    audio_bytes: i32,
    user_message: &str,
    polished: &str,
) {
    // 16 kHz mono PCM16 ⇒ 32000 bytes/sec. Rough cost only; exact billing is a
    // later concern — this ledger exists so cost is observable from day one.
    let audio_seconds = (audio_bytes as i64) / 32_000;
    let deepgram_cost_micros = audio_seconds * 72;
    let _ = sqlx::query(
        "INSERT INTO provider_usage
            (account_id, run_id, provider, operation, input_units, output_units, cost_micros)
         VALUES ($1, $2, 'deepgram', 'stt', $3, 0, $4)",
    )
    .bind(account_id)
    .bind(run_id)
    .bind(audio_seconds as i32)
    .bind(deepgram_cost_micros)
    .execute(db)
    .await;

    let input_tokens = (user_message.len() / 4) as i32;
    let output_tokens = (polished.len() / 4) as i32;
    let groq_cost_micros = (input_tokens as i64 + output_tokens as i64) / 10;
    let _ = sqlx::query(
        "INSERT INTO provider_usage
            (account_id, run_id, provider, operation, input_units, output_units, cost_micros)
         VALUES ($1, $2, 'groq', 'polish', $3, $4, $5)",
    )
    .bind(account_id)
    .bind(run_id)
    .bind(input_tokens)
    .bind(output_tokens)
    .bind(groq_cost_micros)
    .execute(db)
    .await;
}
