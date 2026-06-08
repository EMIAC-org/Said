//! Guest browser audio WebSocket.
//!
//!   GET /v1/meetings/:id/guest-ws?token=<guest-jwt>
//!
//! Receives 16 kHz i16 little-endian PCM frames from the browser, relays them
//! to Deepgram, and feeds final transcripts into the existing MeetingHub path.

use std::{
    collections::VecDeque,
    time::{Duration as StdDuration, Instant},
};

use axum::{
    extract::{Path, Query, State, WebSocketUpgrade, ws::Message},
    http::StatusCode,
    response::IntoResponse,
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;
use tokio_tungstenite::{
    WebSocketStream, connect_async,
    tungstenite::{Message as DgMessage, client::IntoClientRequest},
};
use tracing::{debug, warn};
use uuid::Uuid;

use crate::{AppState, meeting_hub::OutboundMessage};

#[derive(Deserialize)]
pub struct GuestWsQuery {
    pub token: String,
}

pub async fn handler(
    State(state): State<AppState>,
    Path(meeting_id): Path<Uuid>,
    Query(query): Query<GuestWsQuery>,
    ws: WebSocketUpgrade,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let (account_id, display_name, scoped_meeting_id) =
        crate::auth::resolve_guest_ws_token(&query.token, &state)
            .await
            .ok_or_else(|| (StatusCode::UNAUTHORIZED, "invalid or expired token".into()))?;

    if let Some(scoped_meeting_id) = scoped_meeting_id {
        if scoped_meeting_id != meeting_id {
            return Err((
                StatusCode::FORBIDDEN,
                "guest token is not valid for this meeting".into(),
            ));
        }
    }

    let is_participant: bool = sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM meeting_participants
             WHERE meeting_id = $1 AND account_id = $2
        )",
    )
    .bind(meeting_id)
    .bind(account_id)
    .fetch_one(&state.db)
    .await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "database error".into()))?;

    if !is_participant {
        return Err((
            StatusCode::FORBIDDEN,
            "you are not a participant of this meeting".into(),
        ));
    }

    let email = sqlx::query_scalar::<_, String>("SELECT email FROM accounts WHERE id = $1")
        .bind(account_id)
        .fetch_one(&state.db)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "database error".into()))?;

    Ok(ws.on_upgrade(move |socket| async move {
        let db = state.db.clone();
        let hub = state.hub.clone();
        let deepgram_api_key = state.deepgram_api_key.clone();

        let mut outbound_rx = hub
            .join(
                meeting_id,
                account_id,
                email,
                display_name.clone(),
                &db,
            )
            .await;

        let (mut browser_sink, mut browser_stream) = socket.split();

        let welcome = json!({
            "type": "connected",
            "meeting_id": meeting_id.to_string(),
            "account_id": account_id.to_string(),
            "display_name": display_name,
        });
        if browser_sink
            .send(Message::Text(welcome.to_string()))
            .await
            .is_err()
        {
            hub.leave(meeting_id, account_id, &db).await;
            return;
        }

        if send_catchup(&mut browser_sink, meeting_id, &db).await.is_err() {
            hub.leave(meeting_id, account_id, &db).await;
            return;
        }

        if deepgram_api_key.trim().is_empty() {
            let err = json!({"type": "error", "code": "stt_unavailable"}).to_string();
            let _ = browser_sink.send(Message::Text(err)).await;
            hub.leave(meeting_id, account_id, &db).await;
            return;
        }

        let dg_socket = match connect_deepgram(&deepgram_api_key).await {
            Ok(socket) => socket,
            Err(e) => {
                warn!("[guest-ws] deepgram connect failed: {e}");
                let err = json!({"type": "error", "code": "stt_unavailable"}).to_string();
                let _ = browser_sink.send(Message::Text(err)).await;
                hub.leave(meeting_id, account_id, &db).await;
                return;
            }
        };
        let (mut dg_sink, mut dg_stream) = dg_socket.split();

        let stream_started_at = Instant::now();
        let mut muted = false;
        let mut audio_buffer: VecDeque<(Instant, Vec<u8>)> = VecDeque::new();

        loop {
            tokio::select! {
                Some(browser_msg) = browser_stream.next() => {
                    match browser_msg {
                        Ok(Message::Binary(_)) if muted => {}
                        Ok(Message::Binary(pcm)) => {
                            let pcm = pcm.to_vec();
                            remember_audio(&mut audio_buffer, pcm.clone());
                            match dg_sink.send(DgMessage::Binary(pcm)).await {
                                Ok(()) => {}
                                Err(_) => {
                                    warn!("[guest-ws] failed to forward browser audio to deepgram");
                                    let err = json!({"type": "status", "code": "stt_reconnecting"}).to_string();
                                    let _ = browser_sink.send(Message::Text(err)).await;
                                    match reconnect_deepgram(&deepgram_api_key, &audio_buffer).await {
                                        Ok((new_sink, new_stream)) => {
                                            dg_sink = new_sink;
                                            dg_stream = new_stream;
                                            let ok = json!({"type": "status", "code": "stt_reconnected"}).to_string();
                                            let _ = browser_sink.send(Message::Text(ok)).await;
                                        }
                                        Err(e) => {
                                            warn!("[guest-ws] deepgram reconnect failed: {e}");
                                            let err = json!({"type": "error", "code": "stt_disconnected"}).to_string();
                                            let _ = browser_sink.send(Message::Text(err)).await;
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                        Ok(Message::Text(text)) => {
                            match handle_browser_control(&text, &mut muted, &mut browser_sink).await {
                                Ok(()) => {}
                                Err(()) => break,
                            }
                        }
                        Ok(Message::Close(_)) => break,
                        Err(e) => {
                            debug!("[guest-ws] browser read error: {e}");
                            break;
                        }
                        _ => {}
                    }
                }

                Some(dg_msg) = dg_stream.next() => {
                    let transcript = match dg_msg {
                        Ok(DgMessage::Text(text)) => extract_final_transcript(&text),
                        Ok(DgMessage::Close(_)) => {
                            warn!("[guest-ws] deepgram closed the stream");
                            let err = json!({"type": "status", "code": "stt_reconnecting"}).to_string();
                            let _ = browser_sink.send(Message::Text(err)).await;
                            match reconnect_deepgram(&deepgram_api_key, &audio_buffer).await {
                                Ok((new_sink, new_stream)) => {
                                    dg_sink = new_sink;
                                    dg_stream = new_stream;
                                    let ok = json!({"type": "status", "code": "stt_reconnected"}).to_string();
                                    let _ = browser_sink.send(Message::Text(ok)).await;
                                    None
                                }
                                Err(e) => {
                                    warn!("[guest-ws] deepgram reconnect failed: {e}");
                                    let err = json!({"type": "error", "code": "stt_disconnected"}).to_string();
                                    let _ = browser_sink.send(Message::Text(err)).await;
                                    break;
                                }
                            }
                        }
                        Err(e) => {
                            warn!("[guest-ws] deepgram read error: {e}");
                            let err = json!({"type": "status", "code": "stt_reconnecting"}).to_string();
                            let _ = browser_sink.send(Message::Text(err)).await;
                            match reconnect_deepgram(&deepgram_api_key, &audio_buffer).await {
                                Ok((new_sink, new_stream)) => {
                                    dg_sink = new_sink;
                                    dg_stream = new_stream;
                                    let ok = json!({"type": "status", "code": "stt_reconnected"}).to_string();
                                    let _ = browser_sink.send(Message::Text(ok)).await;
                                    None
                                }
                                Err(e) => {
                                    warn!("[guest-ws] deepgram reconnect failed: {e}");
                                    let err = json!({"type": "error", "code": "stt_disconnected"}).to_string();
                                    let _ = browser_sink.send(Message::Text(err)).await;
                                    break;
                                }
                            }
                        }
                        _ => None,
                    };

                    if let Some(text) = transcript {
                        let timestamp_ms = stream_started_at.elapsed().as_millis() as i64;
                        if let Some(msg) = hub
                            .ingest_chunk(
                                meeting_id,
                                account_id,
                                display_name.clone(),
                                text,
                                timestamp_ms,
                                &db,
                            )
                            .await
                        {
                            if send_outbound(&mut browser_sink, &msg).await.is_err() {
                                break;
                            }
                        }
                    }
                }

                Some(msg) = outbound_rx.recv() => {
                    if send_outbound(&mut browser_sink, &msg).await.is_err() {
                        break;
                    }
                }

                else => break,
            }
        }

        let _ = dg_sink.close().await;
        hub.leave(meeting_id, account_id, &db).await;
        debug!("[guest-ws] closed: meeting={meeting_id} account={account_id}");
    }))
}

async fn connect_deepgram(
    api_key: &str,
) -> Result<
    WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    Box<dyn std::error::Error + Send + Sync>,
> {
    let url = "wss://api.deepgram.com/v1/listen?model=nova-3&language=hi&smart_format=true&encoding=linear16&sample_rate=16000&channels=1&interim_results=true&endpointing=1000&utterance_end_ms=2000";
    let mut request = url.into_client_request()?;
    request
        .headers_mut()
        .insert("Authorization", format!("Token {api_key}").parse()?);
    let (socket, _) = connect_async(request).await?;
    Ok(socket)
}

async fn reconnect_deepgram(
    api_key: &str,
    audio_buffer: &VecDeque<(Instant, Vec<u8>)>,
) -> Result<
    (
        futures_util::stream::SplitSink<
            WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
            DgMessage,
        >,
        futures_util::stream::SplitStream<
            WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
        >,
    ),
    Box<dyn std::error::Error + Send + Sync>,
> {
    let delays = [
        StdDuration::from_secs(1),
        StdDuration::from_secs(2),
        StdDuration::from_secs(4),
        StdDuration::from_secs(8),
        StdDuration::from_secs(10),
    ];

    let mut last_error: Option<Box<dyn std::error::Error + Send + Sync>> = None;
    for delay in delays {
        tokio::time::sleep(delay).await;
        match connect_deepgram(api_key).await {
            Ok(socket) => {
                let (mut sink, stream) = socket.split();
                for (_, pcm) in audio_buffer {
                    sink.send(DgMessage::Binary(pcm.clone())).await?;
                }
                return Ok((sink, stream));
            }
            Err(e) => {
                last_error = Some(e);
            }
        }
    }

    Err(last_error.unwrap_or_else(|| "deepgram reconnect failed".into()))
}

fn remember_audio(audio_buffer: &mut VecDeque<(Instant, Vec<u8>)>, pcm: Vec<u8>) {
    let now = Instant::now();
    audio_buffer.push_back((now, pcm));
    while audio_buffer.front().is_some_and(|(captured_at, _)| {
        now.duration_since(*captured_at) > StdDuration::from_secs(5)
    }) {
        audio_buffer.pop_front();
    }
}

async fn handle_browser_control<S>(
    text: &str,
    muted: &mut bool,
    browser_sink: &mut S,
) -> Result<(), ()>
where
    S: futures_util::Sink<Message> + Unpin,
{
    let Ok(raw) = serde_json::from_str::<serde_json::Value>(text) else {
        return Ok(());
    };
    match raw.get("type").and_then(|v| v.as_str()) {
        Some("ping") => browser_sink
            .send(Message::Text(json!({"type": "pong"}).to_string()))
            .await
            .map_err(|_| ()),
        Some("mute") => {
            *muted = true;
            browser_sink
                .send(Message::Text(
                    json!({"type": "mute_state", "muted": true}).to_string(),
                ))
                .await
                .map_err(|_| ())
        }
        Some("unmute") => {
            *muted = false;
            browser_sink
                .send(Message::Text(
                    json!({"type": "mute_state", "muted": false}).to_string(),
                ))
                .await
                .map_err(|_| ())
        }
        _ => Ok(()),
    }
}

fn extract_final_transcript(text: &str) -> Option<String> {
    let raw: serde_json::Value = serde_json::from_str(text).ok()?;
    if raw.get("is_final").and_then(|v| v.as_bool()) != Some(true) {
        return None;
    }
    let transcript = raw
        .get("channel")?
        .get("alternatives")?
        .as_array()?
        .first()?
        .get("transcript")?
        .as_str()?
        .trim();
    if transcript.is_empty() {
        None
    } else {
        Some(transcript.to_string())
    }
}

async fn send_outbound<S>(sink: &mut S, msg: &OutboundMessage) -> Result<(), ()>
where
    S: futures_util::Sink<Message> + Unpin,
{
    let json = serde_json::to_string(msg).map_err(|_| ())?;
    sink.send(Message::Text(json)).await.map_err(|_| ())
}

async fn send_catchup<S>(sink: &mut S, meeting_id: Uuid, db: &sqlx::PgPool) -> Result<(), ()>
where
    S: futures_util::Sink<Message> + Unpin,
{
    let summary: Option<String> = sqlx::query_scalar(
        "SELECT summary_text FROM meeting_summaries
          WHERE meeting_id = $1
          ORDER BY updated_at DESC LIMIT 1",
    )
    .bind(meeting_id)
    .fetch_optional(db)
    .await
    .ok()
    .flatten();

    let chunks: Vec<(String, String, String, i64, i32)> = sqlx::query_as(
        "SELECT tc.speaker_id::text,
                COALESCE(NULLIF(om.lark_name, ''), split_part(a.email, '@', 1)),
                tc.text, tc.timestamp_ms, tc.chunk_index
           FROM transcript_chunks tc
           JOIN accounts a ON a.id = tc.speaker_id
           LEFT JOIN org_members om ON om.account_id = tc.speaker_id
                AND om.org_id = (SELECT org_id FROM meetings WHERE id = $1)
          WHERE tc.meeting_id = $1
          ORDER BY tc.chunk_index ASC",
    )
    .bind(meeting_id)
    .fetch_all(db)
    .await
    .unwrap_or_default();

    let chunks_json: Vec<serde_json::Value> = chunks
        .iter()
        .map(|(speaker_id, speaker_name, text, ts, idx)| {
            json!({
                "type": "transcript_chunk",
                "speaker_id": speaker_id,
                "speaker_name": speaker_name,
                "text": text,
                "timestamp_ms": ts,
                "chunk_index": idx,
            })
        })
        .collect();

    let tasks: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT mt.id::text, mt.title,
                COALESCE(NULLIF(om.lark_name, ''), split_part(a.email, '@', 1))
           FROM meeting_tasks mt
           LEFT JOIN accounts a ON a.id = mt.assignee_id
           LEFT JOIN org_members om ON om.account_id = mt.assignee_id
                AND om.org_id = (SELECT org_id FROM meetings WHERE id = $1)
          WHERE mt.meeting_id = $1
          ORDER BY mt.detected_at ASC",
    )
    .bind(meeting_id)
    .fetch_all(db)
    .await
    .unwrap_or_default();

    let tasks_json: Vec<serde_json::Value> = tasks
        .iter()
        .map(|(task_id, title, assignee_name)| {
            json!({
                "task_id": task_id,
                "title": title,
                "assignee_name": assignee_name,
            })
        })
        .collect();

    let decisions: Vec<(String,)> = sqlx::query_as(
        "SELECT text FROM meeting_decisions
          WHERE meeting_id = $1
          ORDER BY detected_at ASC",
    )
    .bind(meeting_id)
    .fetch_all(db)
    .await
    .unwrap_or_default();

    let decisions_json: Vec<serde_json::Value> = decisions
        .iter()
        .map(|(text,)| json!({"text": text}))
        .collect();

    let participants: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT mp.account_id::text,
                a.email,
                COALESCE(NULLIF(om.lark_name, ''), split_part(a.email, '@', 1)),
                mp.status
           FROM meeting_participants mp
           JOIN accounts a ON a.id = mp.account_id
           LEFT JOIN org_members om ON om.account_id = mp.account_id
                AND om.org_id = (SELECT org_id FROM meetings WHERE id = $1)
          WHERE mp.meeting_id = $1
          ORDER BY mp.joined_at ASC NULLS LAST",
    )
    .bind(meeting_id)
    .fetch_all(db)
    .await
    .unwrap_or_default();

    let participants_json: Vec<serde_json::Value> = participants
        .iter()
        .map(|(aid, email, display_name, status)| {
            json!({
                "account_id": aid,
                "email": email,
                "display_name": display_name,
                "status": status,
            })
        })
        .collect();

    let catchup = json!({
        "type": "catchup",
        "summary": summary,
        "current_chunks": chunks_json,
        "tasks": tasks_json,
        "decisions": decisions_json,
        "participants": participants_json,
    });

    sink.send(Message::Text(catchup.to_string()))
        .await
        .map_err(|_| ())
}
