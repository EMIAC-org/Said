//! Sync meeting results (tasks, doc, notifications) to Lark using
//! the tenant-level app_access_token.

use chrono::{DateTime, Duration, FixedOffset, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::lark_client::{LarkEnvelope, get_app_access_token};

// ── Response structs for Lark APIs ─────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct TaskData {
    task: TaskInner,
}

#[derive(Debug, Deserialize)]
struct TaskInner {
    guid: String,
}

#[derive(Debug, Deserialize)]
struct DocData {
    document: DocInner,
}

#[derive(Debug, Deserialize)]
struct DocInner {
    document_id: String,
}

#[derive(Debug, Deserialize)]
struct MessageData {
    message_id: String,
}

#[derive(Debug, Deserialize)]
struct PrimaryCalendarData {
    calendars: Vec<PrimaryCalendarEntry>,
}

#[derive(Debug, Deserialize)]
struct PrimaryCalendarEntry {
    calendar: CalendarInner,
}

#[derive(Debug, Deserialize)]
struct CalendarInner {
    calendar_id: String,
}

#[derive(Debug, Deserialize)]
struct CalendarEventData {
    event: CalendarEventInner,
}

#[derive(Debug, Deserialize)]
struct CalendarEventInner {
    event_id: String,
}

#[derive(Debug, Deserialize)]
struct CalendarAttendeesData {
    attendees: Vec<CalendarAttendeeInner>,
}

#[derive(Debug, Deserialize)]
struct CalendarAttendeeInner {
    attendee_id: Option<String>,
    user_id: Option<String>,
    rsvp_status: Option<String>,
}

// ── Sync result ────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct SyncResult {
    pub tasks_synced: i32,
    pub doc_id: Option<String>,
    pub messages_sent: i32,
}

#[derive(Debug, Clone, Copy)]
pub enum MeetingCardKind {
    Created,
    StartingSoon,
    UrgentJoin,
}

// ── Individual Lark API functions ──────────────────────────────────────────

fn lark_api_base_url() -> String {
    std::env::var("LARK_API_BASE_URL")
        .unwrap_or_else(|_| "https://open.larksuite.com/open-apis".to_string())
        .trim_end_matches('/')
        .to_string()
}

fn ist_offset() -> FixedOffset {
    FixedOffset::east_opt(5 * 3600 + 30 * 60).expect("IST offset is valid")
}

pub fn format_ist_range(
    scheduled_at: Option<DateTime<Utc>>,
    duration_minutes: i32,
) -> Option<String> {
    let start = scheduled_at?;
    let end = start + Duration::minutes(i64::from(duration_minutes.max(1)));
    let offset = ist_offset();
    let start_ist = start.with_timezone(&offset);
    let end_ist = end.with_timezone(&offset);

    Some(format!(
        "{} - {} IST",
        start_ist.format("%d %b %Y, %I:%M %p"),
        end_ist.format("%I:%M %p")
    ))
}

fn build_calendar_event_body(
    title: &str,
    agenda: &str,
    participant_names: &[String],
    scheduled_at: DateTime<Utc>,
    duration_minutes: i32,
) -> serde_json::Value {
    let end_at = scheduled_at + Duration::minutes(i64::from(duration_minutes.max(1)));
    let participant_line = if participant_names.is_empty() {
        "Participants: AirNote meeting participants".to_string()
    } else {
        format!("Participants: {}", participant_names.join(", "))
    };
    let description = if agenda.trim().is_empty() {
        format!("{participant_line}\n\nOpen AirNote to join.")
    } else {
        format!(
            "Agenda:\n{}\n\n{participant_line}\n\nOpen AirNote to join.",
            agenda.trim()
        )
    };

    serde_json::json!({
        "summary": title,
        "description": description,
        "start_time": {
            "timestamp": scheduled_at.timestamp().to_string(),
            "timezone": "Asia/Kolkata"
        },
        "end_time": {
            "timestamp": end_at.timestamp().to_string(),
            "timezone": "Asia/Kolkata"
        },
        "free_busy_status": "busy",
        "visibility": "default",
        "attendee_ability": "can_see_others",
        "reminders": [{ "minutes": 5 }],
        "vchat": { "vc_type": "no_meeting" }
    })
}

/// Resolve the app's primary calendar ID.
pub async fn get_primary_calendar(app_id: &str, app_secret: &str) -> Result<String, String> {
    let token = get_app_access_token(app_id, app_secret).await?;
    let client = reqwest::Client::new();
    let url = format!(
        "{}/calendar/v4/calendars/primary?user_id_type=user_id",
        lark_api_base_url()
    );

    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json; charset=utf-8")
        .send()
        .await
        .map_err(|e| {
            tracing::error!("get_primary_calendar request failed: {e}");
            format!("get_primary_calendar request failed: {e}")
        })?;

    let envelope: LarkEnvelope<PrimaryCalendarData> = resp.json().await.map_err(|e| {
        tracing::error!("get_primary_calendar parse failed: {e}");
        format!("get_primary_calendar parse failed: {e}")
    })?;

    if envelope.code != 0 {
        let msg = envelope.msg.as_deref().unwrap_or("unknown");
        tracing::error!("get_primary_calendar API error {}: {msg}", envelope.code);
        return Err(format!("Lark API error {}: {msg}", envelope.code));
    }

    envelope
        .data
        .and_then(|data| data.calendars.into_iter().next())
        .map(|entry| entry.calendar.calendar_id)
        .ok_or_else(|| "get_primary_calendar: code 0 but no calendar_id".to_string())
}

/// Create a Lark calendar event and return `(calendar_id, event_id)`.
pub async fn create_calendar_event(
    app_id: &str,
    app_secret: &str,
    meeting_id: Uuid,
    title: &str,
    agenda: &str,
    participant_names: &[String],
    scheduled_at: DateTime<Utc>,
    duration_minutes: i32,
) -> Result<(String, String), String> {
    let token = get_app_access_token(app_id, app_secret).await?;
    let calendar_id = get_primary_calendar(app_id, app_secret).await?;
    let body = build_calendar_event_body(
        title,
        agenda,
        participant_names,
        scheduled_at,
        duration_minutes,
    );
    let client = reqwest::Client::new();
    let url = format!(
        "{}/calendar/v4/calendars/{}/events?user_id_type=user_id&idempotency_key={}",
        lark_api_base_url(),
        urlencoding::encode(&calendar_id),
        meeting_id
    );

    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json; charset=utf-8")
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            tracing::error!("create_calendar_event request failed: {e}");
            format!("create_calendar_event request failed: {e}")
        })?;

    let envelope: LarkEnvelope<CalendarEventData> = resp.json().await.map_err(|e| {
        tracing::error!("create_calendar_event parse failed: {e}");
        format!("create_calendar_event parse failed: {e}")
    })?;

    if envelope.code != 0 {
        let msg = envelope.msg.as_deref().unwrap_or("unknown");
        tracing::error!("create_calendar_event API error {}: {msg}", envelope.code);
        return Err(format!("Lark API error {}: {msg}", envelope.code));
    }

    let event_id = envelope
        .data
        .map(|data| data.event.event_id)
        .ok_or_else(|| "create_calendar_event: code 0 but no event_id".to_string())?;

    Ok((calendar_id, event_id))
}

/// Add user attendees to a Lark calendar event.
pub async fn add_calendar_attendees(
    app_id: &str,
    app_secret: &str,
    calendar_id: &str,
    event_id: &str,
    attendee_user_ids: &[String],
) -> Result<usize, String> {
    if attendee_user_ids.is_empty() {
        return Ok(0);
    }

    let token = get_app_access_token(app_id, app_secret).await?;
    let body = serde_json::json!({
        "need_notification": true,
        "attendees": attendee_user_ids.iter().map(|user_id| {
            serde_json::json!({ "type": "user", "user_id": user_id })
        }).collect::<Vec<_>>()
    });
    let client = reqwest::Client::new();
    let url = format!(
        "{}/calendar/v4/calendars/{}/events/{}/attendees?user_id_type=user_id",
        lark_api_base_url(),
        urlencoding::encode(calendar_id),
        urlencoding::encode(event_id)
    );

    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json; charset=utf-8")
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            tracing::error!("add_calendar_attendees request failed: {e}");
            format!("add_calendar_attendees request failed: {e}")
        })?;

    let envelope: LarkEnvelope<CalendarAttendeesData> = resp.json().await.map_err(|e| {
        tracing::error!("add_calendar_attendees parse failed: {e}");
        format!("add_calendar_attendees parse failed: {e}")
    })?;

    if envelope.code != 0 {
        let msg = envelope.msg.as_deref().unwrap_or("unknown");
        tracing::error!("add_calendar_attendees API error {}: {msg}", envelope.code);
        return Err(format!("Lark API error {}: {msg}", envelope.code));
    }

    let attendees = envelope
        .data
        .map(|data| data.attendees)
        .ok_or_else(|| "add_calendar_attendees: code 0 but no attendees".to_string())?;

    for attendee in &attendees {
        tracing::info!(
            "[lark-calendar] attendee added user_id={:?} attendee_id={:?} rsvp={:?}",
            attendee.user_id,
            attendee.attendee_id,
            attendee.rsvp_status
        );
    }

    Ok(attendees.len())
}

/// Create a task in Lark Task v2.
pub async fn create_lark_task(
    app_id: &str,
    app_secret: &str,
    summary: &str,
    description: Option<&str>,
    assignee_lark_user_id: Option<&str>,
) -> Result<String, String> {
    let token = get_app_access_token(app_id, app_secret).await?;

    let mut body = serde_json::json!({
        "summary": summary,
    });

    if let Some(desc) = description {
        body["description"] = serde_json::json!(desc);
    }

    if let Some(user_id) = assignee_lark_user_id {
        body["members"] = serde_json::json!([
            { "id": user_id, "type": "user", "role": "assignee" }
        ]);
    }

    let client = reqwest::Client::new();
    let resp = client
        .post("https://open.larksuite.com/open-apis/task/v2/tasks?user_id_type=user_id")
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json; charset=utf-8")
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            tracing::error!("create_lark_task request failed: {e}");
            format!("create_lark_task request failed: {e}")
        })?;

    let envelope: LarkEnvelope<TaskData> = resp.json().await.map_err(|e| {
        tracing::error!("create_lark_task parse failed: {e}");
        format!("create_lark_task parse failed: {e}")
    })?;

    if envelope.code != 0 {
        let msg = envelope.msg.as_deref().unwrap_or("unknown");
        tracing::error!("create_lark_task API error {}: {msg}", envelope.code);
        return Err(format!("Lark API error {}: {msg}", envelope.code));
    }

    let data = envelope
        .data
        .ok_or_else(|| "create_lark_task: code 0 but no data".to_string())?;

    Ok(data.task.guid)
}

/// Create a Lark Docx document with an explicit bearer token (a user's
/// `access_token` → the doc is owned by that user and lands in their Drive; or
/// an `app_access_token` → owned by the app). `folder_token` is optional;
/// without it the doc goes to the token owner's space root.
pub async fn create_doc_with_token(
    token: &str,
    title: &str,
    folder_token: Option<&str>,
) -> Result<String, String> {
    let mut body = serde_json::json!({ "title": title });
    if let Some(ft) = folder_token {
        body["folder_token"] = serde_json::json!(ft);
    }

    let client = reqwest::Client::new();
    let resp = client
        .post("https://open.larksuite.com/open-apis/docx/v1/documents")
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json; charset=utf-8")
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            tracing::error!("create_doc_with_token request failed: {e}");
            format!("create doc request failed: {e}")
        })?;

    let envelope: LarkEnvelope<DocData> = resp.json().await.map_err(|e| {
        tracing::error!("create_doc_with_token parse failed: {e}");
        format!("create doc parse failed: {e}")
    })?;

    if envelope.code != 0 {
        let msg = envelope.msg.as_deref().unwrap_or("unknown");
        tracing::error!("create_doc_with_token API error {}: {msg}", envelope.code);
        return Err(format!("Lark API error {}: {msg}", envelope.code));
    }

    envelope
        .data
        .map(|data| data.document.document_id)
        .ok_or_else(|| "create doc: code 0 but no data".to_string())
}

/// Create a Lark Docx document owned by the app (tenant token).
pub async fn create_lark_doc(
    app_id: &str,
    app_secret: &str,
    title: &str,
    folder_token: Option<&str>,
) -> Result<String, String> {
    let token = get_app_access_token(app_id, app_secret).await?;
    create_doc_with_token(&token, title, folder_token).await
}

/// Fetch the browseable URL for a docx via the Drive metadata API. Avoids
/// hard-coding a tenant domain — Lark returns the canonical link. Returns
/// Ok(None) if the API doesn't include a URL.
pub async fn fetch_doc_url(token: &str, document_id: &str) -> Result<Option<String>, String> {
    #[derive(serde::Deserialize)]
    struct Metas {
        metas: Vec<Meta>,
    }
    #[derive(serde::Deserialize)]
    struct Meta {
        url: Option<String>,
    }
    let body = serde_json::json!({
        "request_docs": [{ "doc_token": document_id, "doc_type": "docx" }],
        "with_url": true
    });
    let client = reqwest::Client::new();
    let resp = client
        .post("https://open.larksuite.com/open-apis/drive/v1/metas/batch_query")
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json; charset=utf-8")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("fetch_doc_url request failed: {e}"))?;
    let envelope: LarkEnvelope<Metas> = resp
        .json()
        .await
        .map_err(|e| format!("fetch_doc_url parse failed: {e}"))?;
    if envelope.code != 0 {
        let msg = envelope.msg.as_deref().unwrap_or("unknown");
        return Err(format!("Lark API error {}: {msg}", envelope.code));
    }
    Ok(envelope
        .data
        .and_then(|d| d.metas.into_iter().next())
        .and_then(|m| m.url))
}

/// Append structured blocks (heading, text, bullet) to a Lark Docx document.
pub async fn add_doc_content(
    app_id: &str,
    app_secret: &str,
    document_id: &str,
    meeting_title: &str,
    summary: &str,
    tasks: &[(String, Option<String>)],
    decisions: &[String],
    transcript: &[(String, String)],
) -> Result<(), String> {
    let token = get_app_access_token(app_id, app_secret).await?;

    // ── Build blocks ───────────────────────────────────────────────────────
    let mut blocks: Vec<serde_json::Value> = Vec::new();

    // H1: meeting title
    blocks.push(heading_block(3, meeting_title));

    // H2: Summary
    blocks.push(heading_block(4, "Summary"));
    blocks.push(text_block(summary));

    // H2: Action Items (only if non-empty)
    if !tasks.is_empty() {
        blocks.push(heading_block(4, "Action Items"));
        for (title, assignee) in tasks {
            let label = if let Some(name) = assignee {
                format!("{title} \u{2014} {name}")
            } else {
                title.clone()
            };
            blocks.push(bullet_block(&label));
        }
    }

    // H2: Decisions (only if non-empty)
    if !decisions.is_empty() {
        blocks.push(heading_block(4, "Decisions"));
        for d in decisions {
            blocks.push(bullet_block(d));
        }
    }

    // H2: Transcript
    blocks.push(heading_block(4, "Transcript"));
    for (speaker, text) in transcript {
        blocks.push(text_block(&format!("[{speaker}]: {text}")));
    }

    // ── Send in batches of 50 ──────────────────────────────────────────────
    let url = format!(
        "https://open.larksuite.com/open-apis/docx/v1/documents/{document_id}/blocks/{document_id}/children"
    );
    let client = reqwest::Client::new();

    for chunk in blocks.chunks(50) {
        let body = serde_json::json!({ "children": chunk });

        let resp = client
            .post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                tracing::error!("add_doc_content request failed: {e}");
                format!("add_doc_content request failed: {e}")
            })?;

        let envelope: LarkEnvelope<serde_json::Value> = resp.json().await.map_err(|e| {
            tracing::error!("add_doc_content parse failed: {e}");
            format!("add_doc_content parse failed: {e}")
        })?;

        if envelope.code != 0 {
            let msg = envelope.msg.as_deref().unwrap_or("unknown");
            tracing::error!("add_doc_content API error {}: {msg}", envelope.code);
            return Err(format!("Lark API error {}: {msg}", envelope.code));
        }
    }

    Ok(())
}

/// Send a rich-text (post) message to a Lark user.
pub async fn send_lark_message(
    app_id: &str,
    app_secret: &str,
    user_id: &str,
    title: &str,
    body_text: &str,
) -> Result<String, String> {
    let token = get_app_access_token(app_id, app_secret).await?;

    // `content` must be a JSON-stringified string
    let content_inner = serde_json::json!({
        "en_us": {
            "title": title,
            "content": [[{ "tag": "text", "text": body_text }]]
        }
    });
    let content_str = serde_json::to_string(&content_inner).map_err(|e| {
        tracing::error!("send_lark_message serialize content failed: {e}");
        format!("serialize content failed: {e}")
    })?;

    let body = serde_json::json!({
        "receive_id": user_id,
        "msg_type": "post",
        "content": content_str,
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(format!(
            "{}/im/v1/messages?receive_id_type=user_id",
            lark_api_base_url()
        ))
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json; charset=utf-8")
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            tracing::error!("send_lark_message request failed: {e}");
            format!("send_lark_message request failed: {e}")
        })?;

    let envelope: LarkEnvelope<MessageData> = resp.json().await.map_err(|e| {
        tracing::error!("send_lark_message parse failed: {e}");
        format!("send_lark_message parse failed: {e}")
    })?;

    if envelope.code != 0 {
        let msg = envelope.msg.as_deref().unwrap_or("unknown");
        tracing::error!("send_lark_message API error {}: {msg}", envelope.code);
        return Err(format!("Lark API error {}: {msg}", envelope.code));
    }

    let data = envelope
        .data
        .ok_or_else(|| "send_lark_message: code 0 but no data".to_string())?;

    Ok(data.message_id)
}

// ── Orchestrator ───────────────────────────────────────────────────────────

/// Sync an ended meeting's results (tasks, doc, notifications) to Lark.
pub async fn sync_meeting_to_lark(
    app_id: &str,
    app_secret: &str,
    meeting_id: Uuid,
    db: &PgPool,
) -> Result<SyncResult, String> {
    // 1. Fetch meeting title
    let meeting_title: String = sqlx::query_scalar("SELECT title FROM meetings WHERE id = $1")
        .bind(meeting_id)
        .fetch_one(db)
        .await
        .map_err(|e| format!("fetch meeting title: {e}"))?;

    // 2. Fetch latest summary
    let summary: Option<String> =
        sqlx::query_scalar("SELECT summary_text FROM meeting_summaries WHERE meeting_id = $1")
            .bind(meeting_id)
            .fetch_optional(db)
            .await
            .map_err(|e| format!("fetch summary: {e}"))?;
    let summary_text = summary.unwrap_or_default();

    // 3. Fetch tasks with assignee Lark info (join org_members)
    let tasks: Vec<(Uuid, String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT mt.id, mt.title, om.lark_user_id, om.lark_name
           FROM meeting_tasks mt
           LEFT JOIN org_members om ON om.account_id = mt.assignee_id
          WHERE mt.meeting_id = $1
          ORDER BY mt.detected_at",
    )
    .bind(meeting_id)
    .fetch_all(db)
    .await
    .map_err(|e| format!("fetch tasks: {e}"))?;

    // 4. Fetch decisions
    let decisions: Vec<String> = sqlx::query_scalar(
        "SELECT text FROM meeting_decisions WHERE meeting_id = $1 ORDER BY detected_at",
    )
    .bind(meeting_id)
    .fetch_all(db)
    .await
    .map_err(|e| format!("fetch decisions: {e}"))?;

    // 5. Fetch transcript chunks with speaker names
    let transcript_rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT COALESCE(om.lark_name, a.email) AS speaker, tc.text
           FROM transcript_chunks tc
           JOIN accounts a ON a.id = tc.speaker_id
           LEFT JOIN org_members om ON om.account_id = tc.speaker_id
          WHERE tc.meeting_id = $1
          ORDER BY tc.chunk_index",
    )
    .bind(meeting_id)
    .fetch_all(db)
    .await
    .map_err(|e| format!("fetch transcript: {e}"))?;

    // 6. Sync tasks to Lark
    let mut tasks_synced: i32 = 0;
    for (task_id, task_title, lark_user_id, _lark_name) in &tasks {
        if let Some(luid) = lark_user_id {
            match create_lark_task(app_id, app_secret, task_title, None, Some(luid)).await {
                Ok(guid) => {
                    // Update DB with lark_task_id + status
                    let _ = sqlx::query(
                        "UPDATE meeting_tasks SET lark_task_id = $1, status = 'synced' WHERE id = $2",
                    )
                    .bind(&guid)
                    .bind(task_id)
                    .execute(db)
                    .await;

                    tasks_synced += 1;
                }
                Err(e) => {
                    tracing::warn!("failed to sync task {task_id}: {e}");
                }
            }
        }
    }

    // 7. Create Lark Doc with full meeting report
    let doc_title = format!("Meeting Report: {meeting_title}");
    let doc_id = match create_lark_doc(app_id, app_secret, &doc_title, None).await {
        Ok(did) => {
            // Build tasks list for doc content
            let task_pairs: Vec<(String, Option<String>)> = tasks
                .iter()
                .map(|(_id, title, _luid, name)| (title.clone(), name.clone()))
                .collect();

            if let Err(e) = add_doc_content(
                app_id,
                app_secret,
                &did,
                &meeting_title,
                &summary_text,
                &task_pairs,
                &decisions,
                &transcript_rows,
            )
            .await
            {
                tracing::warn!("add_doc_content failed: {e}");
            }

            // Store doc_id in meeting_summaries
            let _ =
                sqlx::query("UPDATE meeting_summaries SET lark_doc_id = $1 WHERE meeting_id = $2")
                    .bind(&did)
                    .bind(meeting_id)
                    .execute(db)
                    .await;

            Some(did)
        }
        Err(e) => {
            tracing::warn!("create_lark_doc failed: {e}");
            None
        }
    };

    // 8. Send notifications to task assignees
    let mut messages_sent: i32 = 0;
    // Deduplicate by lark_user_id to avoid spamming the same person
    let mut notified: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (_task_id, task_title, lark_user_id, _lark_name) in &tasks {
        if let Some(luid) = lark_user_id {
            if notified.contains(luid) {
                continue;
            }
            let msg_title = format!("New Task from Meeting: {meeting_title}");
            let msg_body = format!("You've been assigned: {task_title}");
            match send_lark_message(app_id, app_secret, luid, &msg_title, &msg_body).await {
                Ok(_mid) => {
                    notified.insert(luid.clone());
                    messages_sent += 1;
                }
                Err(e) => {
                    tracing::warn!("send_lark_message to {luid} failed: {e}");
                }
            }
        }
    }

    Ok(SyncResult {
        tasks_synced,
        doc_id,
        messages_sent,
    })
}

// ── Pre-meeting notification (Tier 1 + Tier 2) ───────────────────────────

/// Send a Lark interactive card message to a user.
pub async fn send_interactive_card(
    app_id: &str,
    app_secret: &str,
    user_id: &str,
    card: &serde_json::Value,
) -> Result<String, String> {
    let token = get_app_access_token(app_id, app_secret).await?;

    let content_str = serde_json::to_string(card).map_err(|e| {
        tracing::error!("send_interactive_card serialize failed: {e}");
        format!("serialize card failed: {e}")
    })?;

    let body = serde_json::json!({
        "receive_id": user_id,
        "msg_type": "interactive",
        "content": content_str,
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(format!(
            "{}/im/v1/messages?receive_id_type=user_id",
            lark_api_base_url()
        ))
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json; charset=utf-8")
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            tracing::error!("send_interactive_card request failed: {e}");
            format!("send_interactive_card request failed: {e}")
        })?;

    let envelope: LarkEnvelope<MessageData> = resp.json().await.map_err(|e| {
        tracing::error!("send_interactive_card parse failed: {e}");
        format!("send_interactive_card parse failed: {e}")
    })?;

    if envelope.code != 0 {
        let msg = envelope.msg.as_deref().unwrap_or("unknown");
        tracing::error!("send_interactive_card API error {}: {msg}", envelope.code);
        return Err(format!("Lark API error {}: {msg}", envelope.code));
    }

    let data = envelope
        .data
        .ok_or_else(|| "send_interactive_card: code 0 but no data".to_string())?;

    Ok(data.message_id)
}

/// Tier 2: Query open/pending tasks assigned to a participant from previous meetings.
async fn get_open_tasks_for_participant(
    account_id: Uuid,
    org_id: Uuid,
    db: &PgPool,
) -> Vec<(String, String)> {
    let rows: Result<Vec<(String, String)>, _> = sqlx::query_as(
        "SELECT mt.title, m.title
           FROM meeting_tasks mt
           JOIN meetings m ON m.id = mt.meeting_id
          WHERE mt.assignee_id = $1
            AND mt.status IN ('pending', 'synced')
            AND m.org_id = $2
          ORDER BY mt.detected_at DESC
          LIMIT 5",
    )
    .bind(account_id)
    .bind(org_id)
    .fetch_all(db)
    .await;

    rows.unwrap_or_default()
}

/// Tier 2: Generate a suggested agenda from meeting title + participant names using Codex.
async fn generate_agenda_suggestion(
    access_token: &str,
    meeting_title: &str,
    participant_names: &[String],
) -> Result<String, String> {
    let names_str = participant_names.join(", ");
    let instructions = "You are a meeting preparation assistant. Given the meeting title and participants, suggest a concise agenda (3-5 bullet points). Respond with ONLY the agenda text using bullet points. Keep it brief and actionable.";
    let user_input =
        format!("Meeting: {meeting_title}\nParticipants: {names_str}\n\nSuggest an agenda:");

    match crate::codex_client::call_codex(access_token, "gpt-4.1-mini", instructions, &user_input)
        .await
    {
        Ok(resp) => {
            let text = resp.text.trim().to_string();
            if text.is_empty() {
                Err("empty agenda response".to_string())
            } else {
                Ok(text)
            }
        }
        Err(e) => Err(format!("codex agenda generation failed: {e}")),
    }
}

/// Build a Lark interactive card for meeting notification.
pub fn build_meeting_card(
    title: &str,
    agenda: &str,
    participant_names: &[String],
    open_tasks: &[(String, String)],
    scheduled_at: Option<DateTime<Utc>>,
    duration_minutes: i32,
    kind: MeetingCardKind,
) -> serde_json::Value {
    let mut elements: Vec<serde_json::Value> = Vec::new();
    let (header_template, header_title, lead) = match kind {
        MeetingCardKind::Created => (
            "blue",
            "Meeting scheduled",
            "An AirNote meeting has been scheduled.",
        ),
        MeetingCardKind::StartingSoon => {
            ("blue", "Starting soon", "This AirNote meeting starts soon.")
        }
        MeetingCardKind::UrgentJoin => (
            "red",
            "Meeting is live",
            "This AirNote meeting is live and you have not joined yet.",
        ),
    };

    elements.push(serde_json::json!({
        "tag": "div",
        "text": { "content": format!("**{lead}**\n**Meeting:** {title}"), "tag": "lark_md" }
    }));

    if let Some(when) = format_ist_range(scheduled_at, duration_minutes) {
        elements.push(serde_json::json!({
            "tag": "div",
            "text": { "content": format!("**When:** {when}"), "tag": "lark_md" }
        }));
    }

    if !agenda.trim().is_empty() {
        elements.push(serde_json::json!({
            "tag": "div",
            "text": { "content": format!("**Agenda:**\n{}", agenda.trim()), "tag": "lark_md" }
        }));
    }

    if !participant_names.is_empty() {
        let names = participant_names.join(", ");
        elements.push(serde_json::json!({
            "tag": "div",
            "text": { "content": format!("**Participants:** {names}"), "tag": "lark_md" }
        }));
    }

    // Carry-forward open tasks (Tier 2)
    if !open_tasks.is_empty() {
        elements.push(serde_json::json!({ "tag": "hr" }));
        let mut tasks_md = format!(
            "**You have {} open item(s) from previous meetings:**\n",
            open_tasks.len()
        );
        for (task_title, meeting_title) in open_tasks {
            tasks_md.push_str(&format!("- {task_title} (from *{meeting_title}*)\n"));
        }
        elements.push(serde_json::json!({
            "tag": "div",
            "text": { "content": tasks_md, "tag": "lark_md" }
        }));
    }

    elements.push(serde_json::json!({ "tag": "hr" }));
    elements.push(serde_json::json!({
        "tag": "note",
        "elements": [{
            "tag": "plain_text",
            "content": "Open AirNote to join the meeting"
        }]
    }));

    serde_json::json!({
        "config": { "wide_screen_mode": true },
        "header": {
            "title": { "content": header_title, "tag": "plain_text" },
            "template": header_template
        },
        "elements": elements
    })
}

pub async fn send_meeting_card(
    app_id: &str,
    app_secret: &str,
    user_id: &str,
    title: &str,
    agenda: &str,
    participant_names: &[String],
    open_tasks: &[(String, String)],
    scheduled_at: Option<DateTime<Utc>>,
    duration_minutes: i32,
    kind: MeetingCardKind,
) -> Result<String, String> {
    let card = build_meeting_card(
        title,
        agenda,
        participant_names,
        open_tasks,
        scheduled_at,
        duration_minutes,
        kind,
    );

    send_interactive_card(app_id, app_secret, user_id, &card).await
}

/// Send pre-meeting notifications to all participants via Lark interactive cards.
/// Includes Tier 2: carry-forward tasks + smart agenda generation.
/// If `scheduled_at` is within 5 minutes (or null), also sends the "starting soon"
/// reminder immediately so the background worker doesn't need to catch it.
pub async fn send_meeting_notification(
    app_id: &str,
    app_secret: &str,
    meeting_id: Uuid,
    db: &PgPool,
) -> Result<i32, String> {
    // Fetch meeting info including scheduled_at
    let row: Option<(
        String,
        Option<String>,
        Uuid,
        Option<DateTime<Utc>>,
        i32,
        Option<String>,
    )> = sqlx::query_as(
        "SELECT title, agenda, org_id, scheduled_at, duration_minutes, lark_event_id
           FROM meetings
          WHERE id = $1",
    )
    .bind(meeting_id)
    .fetch_optional(db)
    .await
    .map_err(|e| format!("fetch meeting: {e}"))?;

    let Some((title, agenda_opt, org_id, scheduled_at, duration_minutes, existing_event_id)) = row
    else {
        return Err("meeting not found".to_string());
    };

    // Check if meeting is imminent (within 5 minutes or no scheduled time = "start now")
    let is_imminent = match scheduled_at {
        None => true,
        Some(at) => {
            let mins_until = (at - chrono::Utc::now()).num_minutes();
            mins_until < 5
        }
    };

    // Fetch participants with lark_user_id
    let participants: Vec<(Uuid, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT mp.account_id, om.lark_user_id, om.lark_name
           FROM meeting_participants mp
           JOIN org_members om ON om.account_id = mp.account_id AND om.org_id = $2
          WHERE mp.meeting_id = $1
            AND om.lark_user_id IS NOT NULL",
    )
    .bind(meeting_id)
    .bind(org_id)
    .fetch_all(db)
    .await
    .map_err(|e| format!("fetch participants: {e}"))?;

    if participants.is_empty() {
        tracing::info!("[lark-notify] no participants with lark_user_id for meeting {meeting_id}");
        return Ok(0);
    }

    let participant_names: Vec<String> = participants
        .iter()
        .filter_map(|(_, _, name)| name.clone())
        .collect();

    // Tier 2: Smart agenda — if agenda is blank, try generating one
    let mut agenda = agenda_opt.unwrap_or_default();
    if agenda.trim().is_empty() {
        let oai_token: Option<String> = sqlx::query_scalar(
            "SELECT openai_access_token FROM orgs WHERE id = $1 AND openai_access_token IS NOT NULL",
        )
        .bind(org_id)
        .fetch_optional(db)
        .await
        .unwrap_or(None);

        if let Some(token) = oai_token {
            match generate_agenda_suggestion(&token, &title, &participant_names).await {
                Ok(generated) => {
                    tracing::info!("[lark-notify] generated agenda for meeting {meeting_id}");
                    let _ = sqlx::query(
                        "UPDATE meetings SET agenda = $1 WHERE id = $2 AND (agenda IS NULL OR agenda = '')",
                    )
                    .bind(&generated)
                    .bind(meeting_id)
                    .execute(db)
                    .await;
                    agenda = generated;
                }
                Err(e) => {
                    tracing::warn!("[lark-notify] agenda generation failed: {e}");
                }
            }
        }
    }

    if let (Some(start_at), None) = (scheduled_at, existing_event_id.as_ref()) {
        let attendee_user_ids: Vec<String> = participants
            .iter()
            .filter_map(|(_, lark_user_id, _)| lark_user_id.clone())
            .collect();

        match create_calendar_event(
            app_id,
            app_secret,
            meeting_id,
            &title,
            &agenda,
            &participant_names,
            start_at,
            duration_minutes,
        )
        .await
        {
            Ok((calendar_id, event_id)) => {
                if let Err(e) = add_calendar_attendees(
                    app_id,
                    app_secret,
                    &calendar_id,
                    &event_id,
                    &attendee_user_ids,
                )
                .await
                {
                    tracing::warn!(
                        "[lark-calendar] event {event_id} created but attendee add failed: {e}"
                    );
                }

                let _ = sqlx::query(
                    "UPDATE meetings
                        SET lark_calendar_id = $1,
                            lark_event_id = $2,
                            lark_event_status = 'created'
                      WHERE id = $3",
                )
                .bind(&calendar_id)
                .bind(&event_id)
                .bind(meeting_id)
                .execute(db)
                .await;
            }
            Err(e) => {
                tracing::warn!("[lark-calendar] failed to create event for {meeting_id}: {e}");
                let _ =
                    sqlx::query("UPDATE meetings SET lark_event_status = 'failed' WHERE id = $1")
                        .bind(meeting_id)
                        .execute(db)
                        .await;
            }
        }
    }

    // Send card to each participant
    let mut sent = 0i32;
    for (account_id, lark_user_id, _name) in &participants {
        let Some(luid) = lark_user_id else { continue };
        let already_sent: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM meeting_notifications
              WHERE meeting_id = $1 AND account_id = $2 AND notification_type = 'meeting_created'",
        )
        .bind(meeting_id)
        .bind(account_id)
        .fetch_optional(db)
        .await
        .unwrap_or(None);
        if already_sent.is_some() {
            continue;
        }

        // Tier 2: Carry-forward tasks for this participant
        let open_tasks = get_open_tasks_for_participant(*account_id, org_id, db).await;

        match send_meeting_card(
            app_id,
            app_secret,
            luid,
            &title,
            &agenda,
            &participant_names,
            &open_tasks,
            scheduled_at,
            duration_minutes,
            MeetingCardKind::Created,
        )
        .await
        {
            Ok(msg_id) => {
                // Track notification
                let _ = sqlx::query(
                    "INSERT INTO meeting_notifications (meeting_id, account_id, notification_type, lark_message_id)
                     VALUES ($1, $2, 'meeting_created', $3)
                     ON CONFLICT (meeting_id, account_id, notification_type) DO NOTHING",
                )
                .bind(meeting_id)
                .bind(account_id)
                .bind(&msg_id)
                .execute(db)
                .await;
                sent += 1;
            }
            Err(e) => {
                tracing::warn!("[lark-notify] failed to send card to {luid}: {e}");
            }
        }
    }

    // If meeting is imminent (<5 min or "start now"), also send the "starting soon" card
    if is_imminent {
        tracing::info!("[lark-notify] meeting {meeting_id} is imminent — sending reminder cards");
        for (account_id, lark_user_id, _name) in &participants {
            let Some(luid) = lark_user_id else { continue };
            let already_sent: Option<Uuid> = sqlx::query_scalar(
                "SELECT id FROM meeting_notifications
                  WHERE meeting_id = $1 AND account_id = $2 AND notification_type = 'reminder_5m'",
            )
            .bind(meeting_id)
            .bind(account_id)
            .fetch_optional(db)
            .await
            .unwrap_or(None);
            if already_sent.is_some() {
                continue;
            }

            if let Ok(msg_id) = send_meeting_card(
                app_id,
                app_secret,
                luid,
                &title,
                &agenda,
                &participant_names,
                &[],
                scheduled_at,
                duration_minutes,
                MeetingCardKind::StartingSoon,
            )
            .await
            {
                let _ = sqlx::query(
                    "INSERT INTO meeting_notifications (meeting_id, account_id, notification_type, lark_message_id)
                     VALUES ($1, $2, 'reminder_5m', $3)
                     ON CONFLICT (meeting_id, account_id, notification_type) DO NOTHING",
                )
                .bind(meeting_id)
                .bind(account_id)
                .bind(&msg_id)
                .execute(db)
                .await;
            }
        }
    }

    tracing::info!("[lark-notify] sent {sent} meeting cards for {meeting_id}");
    Ok(sent)
}

// ── Block helpers ──────────────────────────────────────────────────────────

fn heading_block(block_type: u8, text: &str) -> serde_json::Value {
    let key = match block_type {
        3 => "heading1",
        4 => "heading2",
        5 => "heading3",
        _ => "heading2",
    };
    serde_json::json!({
        "block_type": block_type,
        key: {
            "elements": [{ "text_run": { "content": text } }]
        }
    })
}

fn text_block(text: &str) -> serde_json::Value {
    serde_json::json!({
        "block_type": 2,
        "text": {
            "elements": [{ "text_run": { "content": text } }]
        }
    })
}

fn bullet_block(text: &str) -> serde_json::Value {
    // block_type 12 (bullet) MUST use the "bullet" key — using "text" makes Lark
    // reject the whole children batch with "invalid param" (1770001). Lark renders
    // the bullet glyph itself, so no manual "•" prefix.
    serde_json::json!({
        "block_type": 12,
        "bullet": {
            "elements": [{ "text_run": { "content": text } }]
        }
    })
}

/// A horizontal divider block.
fn divider_block() -> serde_json::Value {
    serde_json::json!({ "block_type": 22, "divider": {} })
}

/// Parse one line of lightweight Markdown into Docx text_run elements, honoring
/// `**bold**` spans. Other inline markers are left as literal text. Splitting on
/// `**` flips a bold toggle: even segments are normal, odd segments are bold.
fn inline_elements(text: &str) -> Vec<serde_json::Value> {
    let mut elements = Vec::new();
    let mut bold = false;
    for segment in text.split("**") {
        if !segment.is_empty() {
            elements.push(serde_json::json!({
                "text_run": {
                    "content": segment,
                    "text_element_style": { "bold": bold }
                }
            }));
        }
        bold = !bold;
    }
    if elements.is_empty() {
        elements.push(serde_json::json!({ "text_run": { "content": "" } }));
    }
    elements
}

/// A heading/paragraph block with inline `**bold**` rendered.
fn rich_block(block_type: u8, key: &str, text: &str) -> serde_json::Value {
    serde_json::json!({
        "block_type": block_type,
        key: { "elements": inline_elements(text) }
    })
}

/// A bullet block (block_type 12 + the required "bullet" key) with inline
/// `**bold**`. Lark renders the bullet glyph, so the content carries no "•".
fn bullet_rich_block(text: &str) -> serde_json::Value {
    serde_json::json!({ "block_type": 12, "bullet": { "elements": inline_elements(text) } })
}

/// Render the LLM's lightweight Markdown summary into Docx blocks: ATX headings
/// (`#`/`##`/`###`), bullet lists (`-`/`*`/`+`/`•`), and paragraphs, with
/// `**bold**` spans. Blank lines are dropped. This makes the MoM read like a
/// real document using only the proven block-insert path (no markdown-convert
/// API dependency).
fn markdown_to_blocks(md: &str) -> Vec<serde_json::Value> {
    let mut blocks = Vec::new();
    for raw in md.lines() {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("### ") {
            blocks.push(rich_block(5, "heading3", rest.trim()));
        } else if let Some(rest) = trimmed.strip_prefix("## ") {
            blocks.push(rich_block(4, "heading2", rest.trim()));
        } else if let Some(rest) = trimmed.strip_prefix("# ") {
            // Demote H1 so it doesn't compete with the document title.
            blocks.push(rich_block(4, "heading2", rest.trim()));
        } else if let Some(rest) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
            .or_else(|| trimmed.strip_prefix("+ "))
            .or_else(|| trimmed.strip_prefix("\u{2022} "))
        {
            blocks.push(bullet_rich_block(rest.trim()));
        } else {
            blocks.push(rich_block(2, "text", trimmed));
        }
    }
    blocks
}

/// Build the full block list for a beautiful meeting-minutes doc: title,
/// metadata line, divider, summary (rendered Markdown), Action Items, Decisions.
/// Pure (no I/O) so it can be unit-tested.
pub fn build_minutes_blocks(
    title: &str,
    meta_line: &str,
    summary_md: &str,
    action_items: &[(String, Option<String>)],
    decisions: &[String],
) -> Vec<serde_json::Value> {
    let mut blocks = vec![heading_block(3, title)];
    if !meta_line.trim().is_empty() {
        blocks.push(text_block(meta_line));
    }
    blocks.push(divider_block());

    if !summary_md.trim().is_empty() {
        blocks.push(heading_block(4, "\u{1F4CC} Summary"));
        blocks.extend(markdown_to_blocks(summary_md));
    }

    if !action_items.is_empty() {
        blocks.push(heading_block(4, "\u{2705} Action Items"));
        for (item_title, owner) in action_items {
            let line = match owner {
                Some(o) if !o.trim().is_empty() => {
                    format!("**{}** \u{2014} {}", o.trim(), item_title.trim())
                }
                _ => item_title.trim().to_string(),
            };
            blocks.push(bullet_rich_block(&line));
        }
    }

    if !decisions.is_empty() {
        blocks.push(heading_block(4, "\u{1F3AF} Decisions"));
        for d in decisions {
            if !d.trim().is_empty() {
                blocks.push(bullet_rich_block(d.trim()));
            }
        }
    }

    blocks
}

/// Insert pre-built blocks into a Docx document with an explicit bearer token
/// (batches of 50 children).
pub async fn insert_blocks_with_token(
    token: &str,
    document_id: &str,
    blocks: &[serde_json::Value],
) -> Result<(), String> {
    if blocks.is_empty() {
        return Ok(());
    }
    let url = format!(
        "https://open.larksuite.com/open-apis/docx/v1/documents/{document_id}/blocks/{document_id}/children"
    );
    let client = reqwest::Client::new();
    for chunk in blocks.chunks(50) {
        let body = serde_json::json!({ "children": chunk });
        let resp = client
            .post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("insert_doc_blocks request failed: {e}"))?;
        let envelope: LarkEnvelope<serde_json::Value> = resp
            .json()
            .await
            .map_err(|e| format!("insert_doc_blocks parse failed: {e}"))?;
        if envelope.code != 0 {
            let msg = envelope.msg.as_deref().unwrap_or("unknown");
            return Err(format!("Lark API error {}: {msg}", envelope.code));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn ist_range_formats_utc_as_india_time() {
        let start = Utc.with_ymd_and_hms(2026, 5, 17, 4, 30, 0).unwrap();

        assert_eq!(
            format_ist_range(Some(start), 30).unwrap(),
            "17 May 2026, 10:00 AM - 10:30 AM IST"
        );
    }

    #[test]
    fn calendar_event_body_matches_lark_dto() {
        let start = Utc.with_ymd_and_hms(2026, 5, 17, 4, 30, 0).unwrap();
        let body = build_calendar_event_body(
            "Sprint Planning",
            "Review goals",
            &["Abhishek".to_string(), "Anish".to_string()],
            start,
            45,
        );

        assert_eq!(body["summary"], "Sprint Planning");
        assert_eq!(
            body["start_time"]["timestamp"],
            start.timestamp().to_string()
        );
        assert_eq!(body["start_time"]["timezone"], "Asia/Kolkata");
        assert_eq!(
            body["end_time"]["timestamp"],
            (start + Duration::minutes(45)).timestamp().to_string()
        );
        assert_eq!(body["end_time"]["timezone"], "Asia/Kolkata");
        assert_eq!(body["free_busy_status"], "busy");
        assert_eq!(body["visibility"], "default");
        assert_eq!(body["attendee_ability"], "can_see_others");
        assert_eq!(body["reminders"][0]["minutes"], 5);
        assert_eq!(body["vchat"]["vc_type"], "no_meeting");
        assert!(
            body["description"]
                .as_str()
                .unwrap()
                .contains("Open AirNote to join")
        );
    }

    #[test]
    fn meeting_card_is_interactive_card_shape() {
        let start = Utc.with_ymd_and_hms(2026, 5, 17, 4, 30, 0).unwrap();
        let card = build_meeting_card(
            "Sprint Planning",
            "Review goals",
            &["Abhishek".to_string()],
            &[],
            Some(start),
            30,
            MeetingCardKind::Created,
        );

        assert_eq!(card["config"]["wide_screen_mode"], true);
        assert_eq!(card["header"]["template"], "blue");
        assert_eq!(card["header"]["title"]["tag"], "plain_text");
        assert!(card["elements"].as_array().unwrap().iter().any(|element| {
            element["text"]["content"]
                .as_str()
                .unwrap_or_default()
                .contains("10:00 AM")
        }));
    }

    #[test]
    fn inline_elements_splits_bold_spans() {
        let els = inline_elements("plain **bold** tail");
        assert_eq!(els.len(), 3);
        assert_eq!(els[0]["text_run"]["content"], "plain ");
        assert_eq!(els[0]["text_run"]["text_element_style"]["bold"], false);
        assert_eq!(els[1]["text_run"]["content"], "bold");
        assert_eq!(els[1]["text_run"]["text_element_style"]["bold"], true);
        // An empty string still yields one (empty) run so a block is never invalid.
        assert_eq!(inline_elements("").len(), 1);
    }

    #[test]
    fn markdown_to_blocks_maps_headings_bullets_and_text() {
        let blocks = markdown_to_blocks("## Section\n- first\n* second\nplain line\n\n### Sub");
        let types: Vec<i64> = blocks
            .iter()
            .map(|b| b["block_type"].as_i64().unwrap())
            .collect();
        // heading2, bullet, bullet, text, heading3 — blank line dropped.
        assert_eq!(types, vec![4, 12, 12, 2, 5]);
        assert_eq!(
            blocks[0]["heading2"]["elements"][0]["text_run"]["content"],
            "Section"
        );
        // Bullets MUST use the "bullet" key, not "text" — Lark rejects "text"
        // (1770001) and fails the entire children batch (empty doc).
        assert!(blocks[1]["bullet"].is_object());
        assert!(blocks[1]["text"].is_null());
    }

    #[test]
    fn build_minutes_blocks_has_title_summary_actions_decisions() {
        let blocks = build_minutes_blocks(
            "Weekly Sync",
            "Jun 15, 2026 10:00 UTC \u{00B7} 30 min",
            "## Recap\n- shipped export",
            &[("Ship the docs".into(), Some("Rahul".into()))],
            &["Use DeepSeek for summaries".into()],
        );
        // First block is the H1 title.
        assert_eq!(blocks[0]["block_type"], 3);
        assert_eq!(
            blocks[0]["heading1"]["elements"][0]["text_run"]["content"],
            "Weekly Sync"
        );
        // The owner is bolded inside the action-item bullet.
        let json = serde_json::to_string(&blocks).unwrap();
        assert!(json.contains("\u{2705} Action Items"));
        assert!(json.contains("\u{1F3AF} Decisions"));
        assert!(json.contains("Rahul"));
        assert!(json.contains("Use DeepSeek for summaries"));
    }
}
