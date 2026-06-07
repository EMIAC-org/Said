//! Runtime gateway routes for the server-side AirNote runtime.
//!
//! This is the iPhone-safe foundation: authenticated session bootstrap,
//! privacy-safe event ingestion, and a dry voice WebSocket skeleton. It does
//! not route desktop traffic through the server runtime.

use axum::{
    Json,
    extract::{
        Multipart, Query, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::StatusCode,
    response::IntoResponse,
};
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use chrono::{DateTime, Duration, Utc};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Instant;
use tokio_tungstenite::tungstenite::Message as DgMessage;
use tracing::{debug, warn};
use uuid::Uuid;

use crate::{AppState, auth::AuthUser, runtime};

const LLM_BASE_URL: &str = "https://api.groq.com/openai/v1/chat/completions";
const LLM_MODEL: &str = "llama-3.1-8b-instant";

const MAX_DEVICE_ID_LEN: usize = 160;
const MAX_CLIENT_REQUEST_ID_LEN: usize = 120;
const MAX_EVENT_ID_LEN: usize = 120;
const MAX_EVENT_TYPE_LEN: usize = 120;
const MAX_LABEL_LEN: usize = 120;
const MAX_FIELD_HINT_LEN: usize = 80;
const MAX_TEXT_CONTEXT_LEN: usize = 2_000;
const MAX_RECORDING_SECONDS: i32 = 60;
const SESSION_TTL_MINUTES: i64 = 15;

type ApiResult<T> = Result<T, (StatusCode, Json<Value>)>;

#[derive(Debug, Deserialize)]
pub struct RuntimeSessionRequest {
    pub client_request_id: String,
    pub device_id: String,
    #[serde(default)]
    pub platform: Option<String>,
    #[serde(default)]
    pub surface: Option<String>,
    pub language_hint: String,
    pub style: String,
    #[serde(default)]
    pub keyboard_context: Option<KeyboardContext>,
    #[serde(default)]
    pub target_app: Option<TargetApp>,
    #[serde(default)]
    pub cursor_context: Option<CursorContext>,
    #[serde(default)]
    pub vocab_snapshot_hash: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct KeyboardContext {
    #[serde(default)]
    pub before_text: String,
    #[serde(default)]
    pub after_text: String,
    #[serde(default)]
    pub selected_text: String,
    #[serde(default)]
    pub host_app_label: String,
    #[serde(default)]
    pub field_hint: String,
}

#[derive(Debug, Deserialize)]
pub struct TargetApp {
    pub label: String,
    #[serde(default)]
    pub bundle_id: Option<String>,
    pub field_hint: String,
}

#[derive(Debug, Deserialize)]
pub struct CursorContext {
    #[serde(default)]
    pub before_text: String,
    #[serde(default)]
    pub after_text: String,
    #[serde(default)]
    pub selected_text: String,
}

#[derive(Debug, Serialize)]
pub struct RuntimeSessionResponse {
    pub schema: &'static str,
    pub session_id: Uuid,
    pub session_token: Uuid,
    pub expires_at: DateTime<Utc>,
    pub streaming_enabled: bool,
    pub current_vocab_hash: String,
    pub voice_ws_url: String,
    pub batch_url: String,
    pub max_recording_seconds: i32,
}

#[derive(Debug, Deserialize)]
pub struct RuntimeEventBody {
    #[serde(default)]
    pub event_id: Option<String>,
    #[serde(default)]
    pub schema: Option<String>,
    #[serde(default)]
    pub occurred_at: Option<DateTime<Utc>>,
    pub device_id: String,
    #[serde(default)]
    pub session_id: Option<Uuid>,
    #[serde(default)]
    pub client_request_id: Option<String>,
    #[serde(default)]
    pub build: Option<String>,
    #[serde(default)]
    pub platform: Option<String>,
    #[serde(default)]
    pub surface: Option<String>,
    pub event_type: String,
    #[serde(default)]
    pub redacted_context: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct VoiceWsQuery {
    pub session_id: Uuid,
    pub session_token: Uuid,
}

pub async fn config(State(state): State<AppState>, user: AuthUser) -> ApiResult<Json<Value>> {
    let org_id = resolve_org_optional(&state, user.account_id).await?;
    let current_vocab_hash = current_vocab_hash(&state, org_id).await?;

    Ok(Json(json!({
        "schema": "airnote.runtime.config.v1",
        "runtime": {
            "mode": "server_first_mobile",
            "voice_ws_path": "/v1/runtime/voice",
            "session_path": "/v1/runtime/sessions",
            "mobile_session_path": "/v1/mobile/sessions",
            "event_path": "/v1/runtime/events",
            "mobile_event_path": "/v1/mobile/events",
            "max_recording_seconds": MAX_RECORDING_SECONDS,
            "streaming_enabled": true,
            "batch_fallback_enabled": false,
            "raw_audio_retention": "none",
            "raw_text_retention": "none",
            "learning_mode": "insert_first_learn_later",
            "status": "dry_voice_ws_ready"
        },
        "account": {
            "id": user.account_id,
            "email": user.email
        },
        "org_id": org_id,
        "current_vocab_hash": current_vocab_hash
    })))
}

pub async fn create_session(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<RuntimeSessionRequest>,
) -> ApiResult<Json<RuntimeSessionResponse>> {
    let device_id = clean_required(&body.device_id, MAX_DEVICE_ID_LEN, "device_id")?;
    let client_request_id = clean_required(
        &body.client_request_id,
        MAX_CLIENT_REQUEST_ID_LEN,
        "client_request_id",
    )?;
    let platform = normalize_choice(body.platform.as_deref(), &["ios", "android"], "ios");
    let surface = normalize_choice(
        body.surface.as_deref(),
        &[
            "ios_keyboard",
            "ios_action_button",
            "android_keyboard",
            "android_bubble",
        ],
        "ios_keyboard",
    );
    let language_hint = normalize_choice(
        Some(&body.language_hint),
        &["auto", "en", "hi", "hinglish"],
        "auto",
    );
    let style = normalize_choice(
        Some(&body.style),
        &["direct", "work", "casual", "email", "notes"],
        "work",
    );
    let org_id = resolve_org_optional(&state, user.account_id).await?;
    let current_vocab_hash = current_vocab_hash(&state, org_id).await?;
    let context_json = redacted_session_context(&body)?;
    let expires_at = Utc::now() + Duration::minutes(SESSION_TTL_MINUTES);

    let (session_id, session_token, expires_at, current_vocab_hash): (
        Uuid,
        Uuid,
        DateTime<Utc>,
        String,
    ) = sqlx::query_as(
        "INSERT INTO runtime_sessions (
            account_id, org_id, device_id, client_request_id, platform, surface,
            language_hint, style, context_json, vocab_snapshot_hash, current_vocab_hash,
            streaming_enabled, max_recording_seconds, expires_at, status
         )
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,true,$12,$13,'created')
         ON CONFLICT (account_id, device_id, client_request_id) DO UPDATE
           SET org_id = EXCLUDED.org_id,
               platform = EXCLUDED.platform,
               surface = EXCLUDED.surface,
               language_hint = EXCLUDED.language_hint,
               style = EXCLUDED.style,
               context_json = EXCLUDED.context_json,
               vocab_snapshot_hash = EXCLUDED.vocab_snapshot_hash,
               current_vocab_hash = EXCLUDED.current_vocab_hash,
               streaming_enabled = EXCLUDED.streaming_enabled,
               max_recording_seconds = EXCLUDED.max_recording_seconds,
               expires_at = EXCLUDED.expires_at,
               status = 'created',
               completed_at = NULL
         RETURNING id, session_token, expires_at, current_vocab_hash",
    )
    .bind(user.account_id)
    .bind(org_id)
    .bind(&device_id)
    .bind(&client_request_id)
    .bind(&platform)
    .bind(&surface)
    .bind(&language_hint)
    .bind(&style)
    .bind(context_json)
    .bind(trim_optional(body.vocab_snapshot_hash, 128))
    .bind(&current_vocab_hash)
    .bind(MAX_RECORDING_SECONDS)
    .bind(expires_at)
    .fetch_one(&state.db)
    .await
    .map_err(db_err)?;

    Ok(Json(RuntimeSessionResponse {
        schema: "airnote.runtime.session.v1",
        session_id,
        session_token,
        expires_at,
        streaming_enabled: true,
        current_vocab_hash,
        voice_ws_url: format!(
            "/v1/runtime/voice?session_id={session_id}&session_token={session_token}"
        ),
        batch_url: "/v1/runtime/voice/batch".into(),
        max_recording_seconds: MAX_RECORDING_SECONDS,
    }))
}

pub async fn ingest_event(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<RuntimeEventBody>,
) -> ApiResult<Json<Value>> {
    let device_id = clean_required(&body.device_id, MAX_DEVICE_ID_LEN, "device_id")?;
    let event_type = clean_required(&body.event_type, MAX_EVENT_TYPE_LEN, "event_type")?;
    let client_event_id = trim_optional(body.event_id, MAX_EVENT_ID_LEN);
    let client_request_id = trim_optional(body.client_request_id, MAX_CLIENT_REQUEST_ID_LEN);
    let build = trim_optional(body.build, 80);
    let platform = normalize_choice(body.platform.as_deref(), &["ios", "android"], "ios");
    let surface = normalize_choice(
        body.surface.as_deref(),
        &[
            "ios_keyboard",
            "ios_action_button",
            "android_keyboard",
            "android_bubble",
        ],
        "ios_keyboard",
    );
    let redacted_context = sanitize_context(body.redacted_context.unwrap_or_else(|| json!({})));

    let org_id = if let Some(session_id) = body.session_id {
        let row: Option<(Option<Uuid>,)> = sqlx::query_as(
            "SELECT org_id FROM runtime_sessions
              WHERE id = $1 AND account_id = $2 AND device_id = $3",
        )
        .bind(session_id)
        .bind(user.account_id)
        .bind(&device_id)
        .fetch_optional(&state.db)
        .await
        .map_err(db_err)?;

        if let Some((org_id,)) = row {
            org_id
        } else {
            return Err((
                StatusCode::NOT_FOUND,
                Json(json!({"error": "runtime session not found"})),
            ));
        }
    } else {
        resolve_org_optional(&state, user.account_id).await?
    };

    sqlx::query(
        "INSERT INTO runtime_events (
            session_id, account_id, org_id, device_id, client_event_id,
            client_request_id, build, platform, surface, event_type,
            redacted_context, occurred_at
         )
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)
         ON CONFLICT (account_id, client_event_id) DO NOTHING",
    )
    .bind(body.session_id)
    .bind(user.account_id)
    .bind(org_id)
    .bind(&device_id)
    .bind(client_event_id)
    .bind(client_request_id)
    .bind(build)
    .bind(&platform)
    .bind(&surface)
    .bind(&event_type)
    .bind(redacted_context)
    .bind(body.occurred_at)
    .execute(&state.db)
    .await
    .map_err(db_err)?;

    Ok(Json(json!({ "ok": true, "accepted": 1 })))
}

pub async fn voice_ws(
    State(state): State<AppState>,
    Query(query): Query<VoiceWsQuery>,
    ws: WebSocketUpgrade,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let row: Option<(Uuid, Uuid, Option<Uuid>, String, String, String, DateTime<Utc>)> =
        sqlx::query_as(
            "SELECT account_id, id, org_id, device_id, language_hint, style, expires_at
               FROM runtime_sessions
              WHERE id = $1 AND session_token = $2",
        )
        .bind(query.session_id)
        .bind(query.session_token)
        .fetch_optional(&state.db)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "database error".to_string()))?;

    let Some((account_id, session_id, org_id, device_id, language, style, expires_at)) = row else {
        return Err((StatusCode::UNAUTHORIZED, "invalid runtime session".to_string()));
    };
    if expires_at < Utc::now() {
        return Err((StatusCode::GONE, "runtime session expired".to_string()));
    }

    let vocab_terms = runtime::memory::load_terms_for_prompt(&state.db, account_id).await;

    let run_id: Uuid = sqlx::query_scalar(
        "INSERT INTO runtime_runs (session_id, account_id, org_id, device_id, mode, status)
         VALUES ($1,$2,$3,$4,'server_stream','stream_open')
         RETURNING id",
    )
    .bind(session_id)
    .bind(account_id)
    .bind(org_id)
    .bind(&device_id)
    .fetch_one(&state.db)
    .await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "database error".to_string()))?;

    let _ = sqlx::query("UPDATE runtime_sessions SET status = 'stream_open' WHERE id = $1")
        .bind(session_id)
        .execute(&state.db)
        .await;

    let st = state.clone();
    Ok(ws.on_upgrade(move |socket| {
        handle_voice_socket(
            st, socket, account_id, session_id, run_id, device_id, language, style, vocab_terms,
        )
    }))
}

/// The real server runtime pipeline over the Lark WS contract:
/// auth.hello/voice.start/audio.frame(pcm_b64|binary)/audio.end →
/// runtime.status / transcript.partial / transcript.final / runtime.done / runtime.error.
#[allow(clippy::too_many_arguments)]
async fn handle_voice_socket(
    state: AppState,
    socket: WebSocket,
    account_id: Uuid,
    session_id: Uuid,
    run_id: Uuid,
    device_id: String,
    language: String,
    style: String,
    vocab_terms: Vec<String>,
) {
    let (mut sink, mut client_stream) = socket.split();
    let started = Instant::now();
    let http = reqwest::Client::new();
    let _ = &device_id; // retained for future per-device traces

    let _ = sink
        .send(Message::Text(
            json!({"type":"runtime.status","run_id":run_id,"session_id":session_id,"phase":"connected"})
                .to_string(),
        ))
        .await;

    let mock = state.deepgram_api_key.trim().is_empty() || state.gateway_api_key.trim().is_empty();

    // ── Phase 1: audio → transcript ──────────────────────────────────────────
    let mut transcript = String::new();
    let mut bytes: i32 = 0;
    let stt_started = Instant::now();

    if mock {
        loop {
            match client_stream.next().await {
                Some(Ok(Message::Binary(b))) => bytes += b.len() as i32,
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
                warn!("[runtime-ws] deepgram connect failed: {e}");
                let _ = sink
                    .send(Message::Text(
                        json!({"type":"runtime.error","run_id":run_id,"error_kind":"stt_unavailable"})
                            .to_string(),
                    ))
                    .await;
                finalize_run_failed(&state.db, run_id, Some(session_id), "stt_unavailable", bytes).await;
                let _ = sink.send(Message::Close(None)).await;
                return;
            }
        };
        let (mut dg_sink, mut dg_stream) = dg.split();
        let _ = sink
            .send(Message::Text(
                json!({"type":"runtime.status","run_id":run_id,"phase":"stt_streaming"}).to_string(),
            ))
            .await;

        'capture: loop {
            tokio::select! {
                client_msg = client_stream.next() => {
                    match client_msg {
                        Some(Ok(Message::Binary(b))) => {
                            bytes += b.len() as i32;
                            let _ = dg_sink.send(DgMessage::Binary(b.to_vec())).await;
                        }
                        Some(Ok(Message::Text(t))) => {
                            if is_stop(&t) {
                                let _ = dg_sink.send(DgMessage::Text(json!({"type":"CloseStream"}).to_string())).await;
                                break 'capture;
                            } else if let Some(pcm) = audio_frame_pcm(&t) {
                                bytes += pcm.len() as i32;
                                let _ = dg_sink.send(DgMessage::Binary(pcm)).await;
                            }
                        }
                        Some(Ok(Message::Close(_))) | None => {
                            let _ = dg_sink.send(DgMessage::Text(json!({"type":"CloseStream"}).to_string())).await;
                            break 'capture;
                        }
                        Some(Ok(_)) => {}
                        Some(Err(_)) => {
                            let _ = dg_sink.send(DgMessage::Text(json!({"type":"CloseStream"}).to_string())).await;
                            break 'capture;
                        }
                    }
                }
                dg_msg = dg_stream.next() => {
                    match dg_msg {
                        Some(Ok(DgMessage::Text(txt))) => {
                            if let Some((piece, is_final)) = runtime::stt::extract_transcript(&txt) {
                                let ev = if is_final { "transcript.final" } else { "transcript.partial" };
                                if is_final { append_transcript(&mut transcript, &piece); }
                                let _ = sink.send(Message::Text(json!({"type":ev,"run_id":run_id,"text":piece}).to_string())).await;
                            }
                        }
                        Some(Ok(DgMessage::Close(_))) | None => break 'capture,
                        Some(Ok(_)) => {}
                        Some(Err(e)) => { warn!("[runtime-ws] deepgram read error: {e}"); break 'capture; }
                    }
                }
            }
        }

        while let Some(dg_msg) = dg_stream.next().await {
            match dg_msg {
                Ok(DgMessage::Text(txt)) => {
                    if let Some((piece, true)) = runtime::stt::extract_transcript(&txt) {
                        append_transcript(&mut transcript, &piece);
                        let _ = sink.send(Message::Text(json!({"type":"transcript.final","run_id":run_id,"text":piece}).to_string())).await;
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
        let _ = sink
            .send(Message::Text(
                json!({"type":"runtime.error","run_id":run_id,"error_kind":"empty_transcript"}).to_string(),
            ))
            .await;
        finalize_run_failed(&state.db, run_id, Some(session_id), "empty_transcript", bytes).await;
        let _ = sink.send(Message::Close(None)).await;
        return;
    }

    // ── Phase 2: polish → guard → protected-term resolver ────────────────────
    let _ = sink
        .send(Message::Text(
            json!({"type":"runtime.status","run_id":run_id,"phase":"polishing"}).to_string(),
        ))
        .await;
    let sys = runtime::prompt::build_system_prompt(&language, &style, &vocab_terms);
    let user = runtime::prompt::build_user_message(&transcript, &language);
    let polish_started = Instant::now();
    let polished_raw = if mock {
        mock_polish(&transcript)
    } else {
        match runtime::polish::polish_once(&http, &state.gateway_api_key, LLM_BASE_URL, LLM_MODEL, &sys, &user).await {
            Ok(p) if !p.trim().is_empty() => p,
            _ => transcript.clone(),
        }
    };
    let polish_ms = polish_started.elapsed().as_millis() as i64;

    let guarded = runtime::script::apply_script_guard(polished_raw.trim(), &language);
    let replacements = runtime::memory::load_replacements(&state.db, account_id).await;
    let blocked = runtime::memory::load_blocked(&state.db, account_id).await;
    let output = runtime::resolver::apply_resolver(&guarded, &replacements, &blocked);
    let total_ms = started.elapsed().as_millis() as i64;

    finalize_run_success(
        &state.db,
        run_id,
        Some(session_id),
        transcript.chars().count() as i32,
        output.chars().count() as i32,
        bytes,
        total_ms,
    )
    .await;
    record_usage(&state.db, account_id, run_id, bytes, &user, &output).await;

    let _ = sink
        .send(Message::Text(
            json!({
                "type":"runtime.done","run_id":run_id,"session_id":session_id,
                "output":output,"transcript":transcript,
                "latency_ms":{"stt":stt_ms,"polish":polish_ms,"total":total_ms},
                "mock":mock
            })
            .to_string(),
        ))
        .await;
    let _ = sink.send(Message::Close(None)).await;
}

/// HTTP batch fallback (Wave 2): `POST /v1/runtime/voice/batch`.
pub async fn dictate_batch(
    State(state): State<AppState>,
    user: AuthUser,
    mut multipart: Multipart,
) -> ApiResult<Json<Value>> {
    let mut audio: Vec<u8> = Vec::new();
    let mut audio_ct = String::new();
    let mut session_id: Option<Uuid> = None;
    let mut device_id = "batch".to_string();
    let mut language = "auto".to_string();
    let mut style = "work".to_string();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|_| bad_req("invalid multipart body"))?
    {
        match field.name().unwrap_or("").to_string().as_str() {
            "audio" => {
                audio_ct = field.content_type().unwrap_or("").to_string();
                audio = field
                    .bytes()
                    .await
                    .map_err(|_| bad_req("audio read failed"))?
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
                language = normalize_choice(Some(&v), &["auto", "en", "hi", "hinglish"], "auto");
            }
            "style" => {
                let v = field.text().await.unwrap_or_default();
                style = normalize_choice(
                    Some(&v),
                    &["direct", "work", "casual", "email", "notes"],
                    "work",
                );
            }
            _ => {
                let _ = field.bytes().await;
            }
        }
    }
    if audio.is_empty() {
        return Err(bad_req("audio field required"));
    }

    let started = Instant::now();
    let mock = state.deepgram_api_key.trim().is_empty() || state.gateway_api_key.trim().is_empty();
    let audio_bytes = audio.len() as i32;
    let http = reqwest::Client::new();

    let run_id: Uuid = sqlx::query_scalar(
        "INSERT INTO runtime_runs (session_id, account_id, device_id, mode, status)
         VALUES ($1,$2,$3,'server_batch','created') RETURNING id",
    )
    .bind(session_id)
    .bind(user.account_id)
    .bind(&device_id)
    .fetch_one(&state.db)
    .await
    .map_err(db_err)?;

    let transcript = if mock {
        mock_transcript(&language)
    } else {
        match runtime::stt::transcribe_batch(&http, &state.deepgram_api_key, audio, &audio_ct, &language)
            .await
        {
            Ok(t) => t,
            Err(e) => {
                warn!("[runtime-batch] stt failed: {e}");
                finalize_run_failed(&state.db, run_id, session_id, "stt_failed", audio_bytes).await;
                return Err((
                    StatusCode::BAD_GATEWAY,
                    Json(json!({"error":"stt_failed","message":e,"request_id":run_id})),
                ));
            }
        }
    };
    let transcript = transcript.trim().to_string();
    if transcript.is_empty() {
        finalize_run_failed(&state.db, run_id, session_id, "empty_transcript", audio_bytes).await;
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"error":"empty_transcript","request_id":run_id})),
        ));
    }

    let vocab_terms = runtime::memory::load_terms_for_prompt(&state.db, user.account_id).await;
    let sys = runtime::prompt::build_system_prompt(&language, &style, &vocab_terms);
    let user_msg = runtime::prompt::build_user_message(&transcript, &language);
    let polished_raw = if mock {
        mock_polish(&transcript)
    } else {
        match runtime::polish::polish_once(&http, &state.gateway_api_key, LLM_BASE_URL, LLM_MODEL, &sys, &user_msg)
            .await
        {
            Ok(p) if !p.trim().is_empty() => p,
            _ => transcript.clone(),
        }
    };
    let guarded = runtime::script::apply_script_guard(polished_raw.trim(), &language);
    let replacements = runtime::memory::load_replacements(&state.db, user.account_id).await;
    let blocked = runtime::memory::load_blocked(&state.db, user.account_id).await;
    let output = runtime::resolver::apply_resolver(&guarded, &replacements, &blocked);
    let total_ms = started.elapsed().as_millis() as i64;

    finalize_run_success(
        &state.db,
        run_id,
        session_id,
        transcript.chars().count() as i32,
        output.chars().count() as i32,
        audio_bytes,
        total_ms,
    )
    .await;
    record_usage(&state.db, user.account_id, run_id, audio_bytes, &user_msg, &output).await;

    Ok(Json(json!({
        "schema":"airnote.runtime.dictate.v1","request_id":run_id,"session_id":session_id,
        "transcript":transcript,"output":output,"polished":output,
        "language":language,"style":style,"latency_ms":total_ms,"mock":mock
    })))
}

#[derive(Debug, Deserialize)]
pub struct FeedbackBody {
    #[serde(default)]
    pub action: Option<String>,
    #[serde(default)]
    pub original: Option<String>,
    #[serde(default)]
    pub user_kept: Option<String>,
    #[serde(default)]
    pub run_id: Option<Uuid>,
}

/// Explicit learning (Wave 5): `POST /v1/runtime/feedback`. Edit-event learning
/// with safety gates; never blocks insertion (runs after the result is inserted).
pub async fn feedback(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<FeedbackBody>,
) -> ApiResult<Json<Value>> {
    let action = body.action.unwrap_or_default();
    if action != "learn_spelling" {
        return Ok(Json(json!({ "ok": true, "learned": Value::Null })));
    }
    let account_id = user.account_id;
    let run_id = body.run_id;

    if let (Some(original), Some(kept)) = (body.original.as_deref(), body.user_kept.as_deref()) {
        if !original.trim().is_empty() && !kept.trim().is_empty() {
            match runtime::learning::analyze_edit(original, kept) {
                runtime::learning::LearnDecision::Learn { spoken, canonical } => {
                    runtime::memory::record_replacement(&state.db, account_id, &spoken, &canonical, "edit").await;
                    runtime::memory::record_term(&state.db, account_id, &canonical).await;
                    runtime::memory::record_learning_event(&state.db, account_id, run_id, "learned_replacement").await;
                    return Ok(Json(json!({ "ok": true, "learned": canonical, "from": spoken })));
                }
                runtime::learning::LearnDecision::Block { spoken } => {
                    runtime::memory::record_blocked(&state.db, account_id, &spoken, "common_word").await;
                    runtime::memory::record_learning_event(&state.db, account_id, run_id, "blocked_unsafe").await;
                    return Ok(Json(json!({ "ok": true, "learned": Value::Null, "blocked": spoken })));
                }
                runtime::learning::LearnDecision::Ignore => {
                    runtime::memory::record_learning_event(&state.db, account_id, run_id, "ignored_formatting").await;
                    return Ok(Json(json!({ "ok": true, "learned": Value::Null })));
                }
            }
        }
    }

    if let Some(kept) = body.user_kept.as_deref() {
        let term = clean_required(kept, 120, "user_kept")?;
        runtime::memory::record_term(&state.db, account_id, &term).await;
        runtime::memory::record_learning_event(&state.db, account_id, run_id, "explicit_term").await;
        return Ok(Json(json!({ "ok": true, "learned": term })));
    }

    Ok(Json(json!({ "ok": true, "learned": Value::Null })))
}

// ── runtime pipeline helpers ──────────────────────────────────────────────────

fn bad_req(message: &str) -> (StatusCode, Json<Value>) {
    (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({ "error": message })))
}

fn append_transcript(acc: &mut String, piece: &str) {
    if piece.is_empty() {
        return;
    }
    if !acc.is_empty() {
        acc.push(' ');
    }
    acc.push_str(piece);
}

fn is_stop(text: &str) -> bool {
    serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|v| v.get("type").and_then(Value::as_str).map(str::to_string))
        .map(|t| {
            matches!(
                t.as_str(),
                "audio.end" | "audio_end" | "voice.stop" | "session.stop" | "session_stop" | "stop"
            )
        })
        .unwrap_or(false)
}

/// Decode a Lark `audio.frame` text message carrying base64 PCM into raw bytes.
fn audio_frame_pcm(text: &str) -> Option<Vec<u8>> {
    let v: Value = serde_json::from_str(text).ok()?;
    if v.get("type").and_then(Value::as_str)? != "audio.frame" {
        return None;
    }
    let b64 = v.get("pcm_b64").and_then(Value::as_str)?;
    B64.decode(b64).ok()
}

fn mock_transcript(language: &str) -> String {
    match language {
        "en" => "send the concise update to rahul tomorrow".to_string(),
        _ => "kal ka update concise banake rahul ko bhej do".to_string(),
    }
}

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

async fn finalize_run_success(
    db: &sqlx::PgPool,
    run_id: Uuid,
    session_id: Option<Uuid>,
    transcript_chars: i32,
    polished_chars: i32,
    bytes: i32,
    latency_ms: i64,
) {
    let _ = sqlx::query(
        "UPDATE runtime_runs
            SET status = 'completed',
                transcript_char_count = $2,
                polished_char_count = $3,
                audio_byte_count = $4,
                latency_ms = $5,
                completed_at = now()
          WHERE id = $1",
    )
    .bind(run_id)
    .bind(transcript_chars)
    .bind(polished_chars)
    .bind(bytes)
    .bind(latency_ms as i32)
    .execute(db)
    .await;
    if let Some(sid) = session_id {
        let _ = sqlx::query(
            "UPDATE runtime_sessions SET status = 'completed', completed_at = now() WHERE id = $1",
        )
        .bind(sid)
        .execute(db)
        .await;
    }
}

async fn finalize_run_failed(
    db: &sqlx::PgPool,
    run_id: Uuid,
    session_id: Option<Uuid>,
    code: &str,
    bytes: i32,
) {
    let _ = sqlx::query(
        "UPDATE runtime_runs
            SET status = 'failed', error_code = $2, audio_byte_count = $3, completed_at = now()
          WHERE id = $1",
    )
    .bind(run_id)
    .bind(code)
    .bind(bytes)
    .execute(db)
    .await;
    if let Some(sid) = session_id {
        let _ = sqlx::query(
            "UPDATE runtime_sessions SET status = 'failed', completed_at = now() WHERE id = $1",
        )
        .bind(sid)
        .execute(db)
        .await;
    }
}

async fn record_usage(
    db: &sqlx::PgPool,
    account_id: Uuid,
    run_id: Uuid,
    audio_bytes: i32,
    user_message: &str,
    output: &str,
) {
    let audio_seconds = (audio_bytes as i64) / 32_000;
    let _ = sqlx::query(
        "INSERT INTO runtime_provider_usage
            (account_id, run_id, provider, operation, input_units, output_units, cost_micros)
         VALUES ($1, $2, 'deepgram', 'stt', $3, 0, $4)",
    )
    .bind(account_id)
    .bind(run_id)
    .bind(audio_seconds as i32)
    .bind(audio_seconds * 72)
    .execute(db)
    .await;

    let input_tokens = (user_message.len() / 4) as i32;
    let output_tokens = (output.len() / 4) as i32;
    let _ = sqlx::query(
        "INSERT INTO runtime_provider_usage
            (account_id, run_id, provider, operation, input_units, output_units, cost_micros)
         VALUES ($1, $2, 'groq', 'polish', $3, $4, $5)",
    )
    .bind(account_id)
    .bind(run_id)
    .bind(input_tokens)
    .bind(output_tokens)
    .bind((input_tokens as i64 + output_tokens as i64) / 10)
    .execute(db)
    .await;
}

async fn resolve_org_optional(state: &AppState, account_id: Uuid) -> ApiResult<Option<Uuid>> {
    sqlx::query_scalar("SELECT org_id FROM org_members WHERE account_id = $1 LIMIT 1")
        .bind(account_id)
        .fetch_optional(&state.db)
        .await
        .map_err(db_err)
}

async fn current_vocab_hash(state: &AppState, org_id: Option<Uuid>) -> ApiResult<String> {
    let Some(org_id) = org_id else {
        return Ok("global-v0".into());
    };

    let hash: Option<String> = sqlx::query_scalar(
        "SELECT bucket_hash FROM org_vocab_releases
          WHERE org_id = $1
          ORDER BY version DESC
          LIMIT 1",
    )
    .bind(org_id)
    .fetch_optional(&state.db)
    .await
    .map_err(db_err)?;

    Ok(hash.unwrap_or_else(|| "global-v0".into()))
}

fn redacted_session_context(body: &RuntimeSessionRequest) -> ApiResult<Value> {
    let keyboard = body.keyboard_context.as_ref();
    let target = body.target_app.as_ref();
    let cursor = body.cursor_context.as_ref();

    let host_app_label = keyboard
        .map(|ctx| ctx.host_app_label.as_str())
        .or_else(|| target.map(|ctx| ctx.label.as_str()))
        .map(|raw| clean_optional(raw, MAX_LABEL_LEN))
        .transpose()?;
    let field_hint = keyboard
        .map(|ctx| ctx.field_hint.as_str())
        .or_else(|| target.map(|ctx| ctx.field_hint.as_str()))
        .map(|raw| clean_optional(raw, MAX_FIELD_HINT_LEN))
        .transpose()?;

    let before_text_chars = keyboard
        .map(|ctx| bounded_char_count(&ctx.before_text))
        .or_else(|| cursor.map(|ctx| bounded_char_count(&ctx.before_text)))
        .unwrap_or(0);
    let after_text_chars = keyboard
        .map(|ctx| bounded_char_count(&ctx.after_text))
        .or_else(|| cursor.map(|ctx| bounded_char_count(&ctx.after_text)))
        .unwrap_or(0);
    let selected_text_chars = keyboard
        .map(|ctx| bounded_char_count(&ctx.selected_text))
        .or_else(|| cursor.map(|ctx| bounded_char_count(&ctx.selected_text)))
        .unwrap_or(0);

    Ok(json!({
        "host_app_label": host_app_label,
        "field_hint": field_hint,
        "target_bundle_known": target.and_then(|ctx| ctx.bundle_id.as_ref()).is_some(),
        "before_text_chars": before_text_chars,
        "after_text_chars": after_text_chars,
        "selected_text_chars": selected_text_chars
    }))
}

fn bounded_char_count(raw: &str) -> usize {
    raw.chars()
        .take(MAX_TEXT_CONTEXT_LEN + 1)
        .count()
        .min(MAX_TEXT_CONTEXT_LEN)
}

async fn insert_stage_event(
    db: &sqlx::PgPool,
    run_id: Uuid,
    session_id: Uuid,
    account_id: Uuid,
    device_id: &str,
    stage: &str,
    status: &str,
    metadata: Value,
) {
    let _ = sqlx::query(
        "INSERT INTO runtime_stage_events (
            run_id, session_id, account_id, device_id, stage, status, metadata
         )
         VALUES ($1,$2,$3,$4,$5,$6,$7)",
    )
    .bind(run_id)
    .bind(session_id)
    .bind(account_id)
    .bind(device_id)
    .bind(stage)
    .bind(status)
    .bind(metadata)
    .execute(db)
    .await;
}

async fn complete_dry_run(
    db: &sqlx::PgPool,
    run_id: Uuid,
    session_id: Uuid,
    account_id: Uuid,
    device_id: &str,
    audio_frame_count: i32,
    audio_byte_count: i32,
) {
    let _ = sqlx::query(
        "UPDATE runtime_runs
            SET status = 'dry_completed',
                audio_frame_count = $2,
                audio_byte_count = $3,
                completed_at = now()
          WHERE id = $1",
    )
    .bind(run_id)
    .bind(audio_frame_count)
    .bind(audio_byte_count)
    .execute(db)
    .await;
    let _ = sqlx::query(
        "UPDATE runtime_sessions
            SET status = 'dry_completed',
                completed_at = now()
          WHERE id = $1",
    )
    .bind(session_id)
    .execute(db)
    .await;

    insert_stage_event(
        db,
        run_id,
        session_id,
        account_id,
        device_id,
        "ws.audio_end",
        "dry_completed",
        json!({
            "audio_frame_count": audio_frame_count,
            "audio_byte_count": audio_byte_count,
            "raw_audio_retention": "none"
        }),
    )
    .await;
}

fn sanitize_context(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, child) in map {
                if is_blocked_key(&key) {
                    continue;
                }
                out.insert(key, sanitize_context(child));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.into_iter().map(sanitize_context).collect()),
        Value::String(text) => Value::String(text.chars().take(500).collect()),
        other => other,
    }
}

fn is_blocked_key(key: &str) -> bool {
    const BLOCKED: &[&str] = &[
        "transcript",
        "polished",
        "raw_transcript",
        "enriched_transcript",
        "audio",
        "api_key",
        "secret",
        "password",
        "token",
        "authorization",
        "user_text",
        "user_kept",
        "ai_output",
        "before_text",
        "after_text",
        "selected_text",
    ];
    let lower = key.to_ascii_lowercase();
    BLOCKED.iter().any(|blocked| lower.contains(blocked))
}

fn clean_required(raw: &str, max_len: usize, name: &str) -> ApiResult<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > max_len {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"error": format!("{name} required and must be <= {max_len} chars")})),
        ));
    }
    Ok(trimmed.to_string())
}

fn clean_optional(raw: &str, max_len: usize) -> ApiResult<String> {
    let trimmed = raw.trim();
    if trimmed.len() > max_len {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"error": format!("field must be <= {max_len} chars")})),
        ));
    }
    Ok(trimmed.to_string())
}

fn trim_optional(raw: Option<String>, max_len: usize) -> Option<String> {
    raw.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.chars().take(max_len).collect())
        }
    })
}

fn normalize_choice(raw: Option<&str>, allowed: &[&str], default: &str) -> String {
    let Some(raw) = raw else {
        return default.to_string();
    };
    let normalized = raw.trim().to_ascii_lowercase();
    if allowed.iter().any(|item| *item == normalized) {
        normalized
    } else {
        default.to_string()
    }
}

fn db_err(err: sqlx::Error) -> (StatusCode, Json<Value>) {
    debug!("[runtime] database error: {err}");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": "database error"})),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_context_counts_text_but_does_not_store_it() {
        let body = RuntimeSessionRequest {
            client_request_id: "req-1".into(),
            device_id: "iphone-1".into(),
            platform: None,
            surface: None,
            language_hint: "hinglish".into(),
            style: "work".into(),
            keyboard_context: Some(KeyboardContext {
                before_text: "raw words before cursor".into(),
                after_text: "raw words after cursor".into(),
                selected_text: "selected secret".into(),
                host_app_label: "Messages".into(),
                field_hint: "reply".into(),
            }),
            target_app: None,
            cursor_context: None,
            vocab_snapshot_hash: None,
        };

        let context = redacted_session_context(&body).unwrap();
        assert_eq!(context["host_app_label"], "Messages");
        assert_eq!(context["field_hint"], "reply");
        assert_eq!(context["before_text_chars"], 23);
        assert_ne!(context.to_string(), "raw words before cursor");
        assert!(!context.to_string().contains("selected secret"));
    }

    #[test]
    fn sanitize_context_removes_raw_text_and_secret_keys_recursively() {
        let sanitized = sanitize_context(json!({
            "latency_ms": 100,
            "transcript": "do not store",
            "nested": {
                "api_key": "secret",
                "host_app_label": "Notes",
                "selected_text": "private"
            }
        }));

        assert_eq!(sanitized["latency_ms"], 100);
        assert_eq!(sanitized["nested"]["host_app_label"], "Notes");
        assert!(sanitized.get("transcript").is_none());
        assert!(sanitized["nested"].get("api_key").is_none());
        assert!(sanitized["nested"].get("selected_text").is_none());
    }

    #[test]
    fn normalize_choice_defaults_unknown_values() {
        assert_eq!(
            normalize_choice(Some("HINGLISH"), &["auto", "hinglish"], "auto"),
            "hinglish"
        );
        assert_eq!(
            normalize_choice(Some("pirate"), &["auto", "hinglish"], "auto"),
            "auto"
        );
    }
}
