//! WebSocket handler for meeting transcript streaming.
//!
//!   GET /v1/meetings/:id/ws?token=<session-token>
//!
//! Validates the session token from the query param, verifies the caller is a
//! participant of the meeting, upgrades to WebSocket, and wires up the
//! MeetingHub for real-time transcript fan-out.

use axum::{
    extract::{Path, Query, State, WebSocketUpgrade, ws::Message},
    http::StatusCode,
    response::IntoResponse,
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;
use tracing::{debug, warn};
use uuid::Uuid;

use crate::AppState;
use crate::meeting_hub::InboundMessage;

// ── Query params ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct WsQuery {
    /// Session token (same as Authorization: Bearer <token>).
    /// WebSocket clients can't set custom headers, so we accept it via query.
    pub token: String,
}

// ── GET /v1/meetings/:id/ws ─────────────────────────────────────────────────

pub async fn handler(
    State(state): State<AppState>,
    Path(meeting_id): Path<Uuid>,
    Query(query): Query<WsQuery>,
    ws: WebSocketUpgrade,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    // ── Validate auth token (session UUID or JWT) ────────────────────────────
    let (account_id, email) = crate::auth::resolve_ws_token(&query.token, &state)
        .await
        .ok_or_else(|| (StatusCode::UNAUTHORIZED, "invalid or expired token".into()))?;

    // ── Verify user is a participant of this meeting ────────────────────────
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

    // ── Look up speaker name from org_members (fallback to email) ──────────
    let speaker_name: String = sqlx::query_scalar(
        "SELECT COALESCE(NULLIF(om.lark_name, ''), split_part($3, '@', 1))
           FROM org_members om
           JOIN meetings m ON m.org_id = om.org_id
          WHERE om.account_id = $1 AND m.id = $2",
    )
    .bind(account_id)
    .bind(meeting_id)
    .bind(&email)
    .fetch_optional(&state.db)
    .await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "database error".into()))?
    .unwrap_or_else(|| email.split('@').next().unwrap_or(&email).to_string());

    debug!("[ws] upgrading: meeting={meeting_id} account={account_id} speaker={speaker_name}");

    // ── Upgrade to WebSocket ────────────────────────────────────────────────
    let hub = state.hub.clone();
    let db = state.db.clone();

    Ok(ws.on_upgrade(move |socket| async move {
        // Join the meeting room via the hub
        let mut outbound_rx = hub
            .join(meeting_id, account_id, email, speaker_name.clone(), &db)
            .await;

        let (mut sink, mut stream) = socket.split();

        // Send welcome message directly before entering the loops
        let welcome = json!({
            "type": "connected",
            "meeting_id": meeting_id.to_string(),
            "account_id": account_id.to_string(),
        });
        if sink
            .send(Message::Text(welcome.to_string().into()))
            .await
            .is_err()
        {
            warn!("[ws] failed to send welcome, dropping connection");
            hub.leave(meeting_id, account_id, &db).await;
            return;
        }

        // ── Late joiner catchup ────────────────────────────────────────────
        // Send a single `catchup` message with the current meeting state so
        // late joiners (or reconnecting clients) can render immediately.
        {
            // Latest summary
            let summary: Option<String> = sqlx::query_scalar(
                "SELECT summary_text FROM meeting_summaries
                  WHERE meeting_id = $1
                  ORDER BY updated_at DESC LIMIT 1",
            )
            .bind(meeting_id)
            .fetch_optional(&db)
            .await
            .ok()
            .flatten();

            // All chunks for this meeting, newest sessions last.
            // Guest speaker names: strip the trailing -{uuid32} suffix from said.guest emails.
            let chunks: Vec<(String, String, String, i64, i32)> = sqlx::query_as(
                "SELECT tc.speaker_id::text,
                        CASE WHEN a.email LIKE '%@said.guest'
                             THEN regexp_replace(split_part(a.email, '@', 1), '-[0-9a-f]{32}$', '')
                             ELSE COALESCE(NULLIF(om.lark_name, ''), split_part(a.email, '@', 1))
                        END,
                        tc.text, tc.timestamp_ms, tc.chunk_index
                   FROM transcript_chunks tc
                   JOIN accounts a ON a.id = tc.speaker_id
                   LEFT JOIN org_members om ON om.account_id = tc.speaker_id
                        AND om.org_id = (SELECT org_id FROM meetings WHERE id = $1)
                  WHERE tc.meeting_id = $1
                  ORDER BY tc.chunk_index ASC",
            )
            .bind(meeting_id)
            .fetch_all(&db)
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

            // Tasks
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
            .fetch_all(&db)
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

            // Decisions
            let decisions: Vec<(String,)> = sqlx::query_as(
                "SELECT text FROM meeting_decisions
                  WHERE meeting_id = $1
                  ORDER BY detected_at ASC",
            )
            .bind(meeting_id)
            .fetch_all(&db)
            .await
            .unwrap_or_default();

            let decisions_json: Vec<serde_json::Value> = decisions
                .iter()
                .map(|(text,)| json!({"text": text}))
                .collect();

            // Participants
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
            .fetch_all(&db)
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

            if sink
                .send(Message::Text(catchup.to_string().into()))
                .await
                .is_err()
            {
                warn!("[ws] failed to send catchup, dropping connection");
                hub.leave(meeting_id, account_id, &db).await;
                return;
            }
        }

        // Spawn outbound loop: hub → WebSocket client.
        // Runs in a separate task so inbound can also write pong to the sink
        // without contention — the outbound task owns the sink exclusively.
        // Inbound pong messages are routed through a local channel.
        let (pong_tx, mut pong_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

        let outbound_handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;

                    // Pong replies from inbound side
                    Some(pong_json) = pong_rx.recv() => {
                        if sink.send(Message::Text(pong_json.into())).await.is_err() {
                            break;
                        }
                    }

                    // Hub broadcast messages
                    Some(msg) = outbound_rx.recv() => {
                        let json_str = match serde_json::to_string(&msg) {
                            Ok(s) => s,
                            Err(e) => {
                                warn!("[ws] failed to serialize outbound message: {e}");
                                continue;
                            }
                        };
                        if sink.send(Message::Text(json_str.into())).await.is_err() {
                            break;
                        }
                    }

                    // Both channels closed
                    else => break,
                }
            }
        });

        // ── Inbound loop: WebSocket client → hub ────────────────────────────
        while let Some(msg_result) = stream.next().await {
            let text = match msg_result {
                Ok(Message::Text(t)) => t,
                Ok(Message::Close(_)) => break,
                Err(e) => {
                    debug!("[ws] read error: {e}");
                    break;
                }
                _ => continue, // ignore binary, ping/pong frames
            };

            // ── Resume protocol — intercept before standard deserialization ──
            if let Ok(raw) = serde_json::from_str::<serde_json::Value>(&text) {
                if raw.get("type").and_then(|v| v.as_str()) == Some("resume") {
                    let last_idx = raw
                        .get("last_chunk_index")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(-1) as i32;

                    debug!("[ws] resume request from {account_id}: last_chunk_index={last_idx}");

                    // Send any chunks the client missed (chunk_index > last_idx)
                    let missed: Vec<(String, String, String, i64, i32)> = sqlx::query_as(
                        "SELECT tc.speaker_id::text,
                                COALESCE(NULLIF(om.lark_name, ''), split_part(a.email, '@', 1)),
                                tc.text, tc.timestamp_ms, tc.chunk_index
                           FROM transcript_chunks tc
                           JOIN accounts a ON a.id = tc.speaker_id
                           LEFT JOIN org_members om ON om.account_id = tc.speaker_id
                                AND om.org_id = (SELECT org_id FROM meetings WHERE id = $1)
                          WHERE tc.meeting_id = $1 AND tc.chunk_index > $2
                          ORDER BY tc.chunk_index ASC",
                    )
                    .bind(meeting_id)
                    .bind(last_idx)
                    .fetch_all(&db)
                    .await
                    .unwrap_or_default();

                    for (sid, sname, txt, ts, idx) in &missed {
                        let chunk_msg = json!({
                            "type": "transcript_chunk",
                            "speaker_id": sid,
                            "speaker_name": sname,
                            "text": txt,
                            "timestamp_ms": ts,
                            "chunk_index": idx,
                        });
                        let _ = pong_tx.send(chunk_msg.to_string());
                    }

                    // Update participant state: re-joined, bump disconnect count
                    let _ = sqlx::query(
                        "UPDATE meeting_participants
                            SET status = 'joined',
                                disconnect_count = disconnect_count + 1,
                                last_chunk_index = $3
                          WHERE meeting_id = $1 AND account_id = $2",
                    )
                    .bind(meeting_id)
                    .bind(account_id)
                    .bind(last_idx)
                    .execute(&db)
                    .await;

                    continue;
                }
            }

            let parsed: InboundMessage = match serde_json::from_str(&text) {
                Ok(m) => m,
                Err(e) => {
                    debug!("[ws] malformed inbound message: {e}");
                    continue;
                }
            };

            match parsed {
                InboundMessage::TranscriptChunk { text, timestamp_ms } => {
                    hub.ingest_chunk(
                        meeting_id,
                        account_id,
                        speaker_name.clone(),
                        text,
                        timestamp_ms,
                        &db,
                    )
                    .await;
                }
                InboundMessage::Ping => {
                    let pong = json!({"type": "pong"}).to_string();
                    if pong_tx.send(pong).is_err() {
                        // outbound task died, stop reading
                        break;
                    }
                }
            }
        }

        // Inbound loop ended — clean up
        drop(pong_tx); // signal outbound task to stop
        outbound_handle.abort(); // ensure it terminates promptly

        hub.leave(meeting_id, account_id, &db).await;

        debug!("[ws] closed: meeting={meeting_id} account={account_id}");
    }))
}
