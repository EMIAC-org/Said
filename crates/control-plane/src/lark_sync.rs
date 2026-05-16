//! Sync meeting results (tasks, doc, notifications) to Lark using
//! the tenant-level app_access_token.

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

// ── Sync result ────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct SyncResult {
    pub tasks_synced: i32,
    pub doc_id: Option<String>,
    pub messages_sent: i32,
}

// ── Individual Lark API functions ──────────────────────────────────────────

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

/// Create a Lark Docx document.
pub async fn create_lark_doc(
    app_id: &str,
    app_secret: &str,
    title: &str,
    folder_token: Option<&str>,
) -> Result<String, String> {
    let token = get_app_access_token(app_id, app_secret).await?;

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
            tracing::error!("create_lark_doc request failed: {e}");
            format!("create_lark_doc request failed: {e}")
        })?;

    let envelope: LarkEnvelope<DocData> = resp.json().await.map_err(|e| {
        tracing::error!("create_lark_doc parse failed: {e}");
        format!("create_lark_doc parse failed: {e}")
    })?;

    if envelope.code != 0 {
        let msg = envelope.msg.as_deref().unwrap_or("unknown");
        tracing::error!("create_lark_doc API error {}: {msg}", envelope.code);
        return Err(format!("Lark API error {}: {msg}", envelope.code));
    }

    let data = envelope
        .data
        .ok_or_else(|| "create_lark_doc: code 0 but no data".to_string())?;

    Ok(data.document.document_id)
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
        .post("https://open.larksuite.com/open-apis/im/v1/messages?receive_id_type=user_id")
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
        .post("https://open.larksuite.com/open-apis/im/v1/messages?receive_id_type=user_id")
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
fn build_meeting_card(
    title: &str,
    agenda: &str,
    participant_names: &[String],
    open_tasks: &[(String, String)],
    header_template: &str,
    header_title: &str,
) -> serde_json::Value {
    let mut elements: Vec<serde_json::Value> = Vec::new();

    // Meeting title
    elements.push(serde_json::json!({
        "tag": "div",
        "text": { "content": format!("**Meeting:** {title}"), "tag": "lark_md" }
    }));

    // Agenda
    if !agenda.is_empty() {
        elements.push(serde_json::json!({
            "tag": "div",
            "text": { "content": format!("**Agenda:**\n{agenda}"), "tag": "lark_md" }
        }));
    }

    // Participants
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
            tasks_md.push_str(&format!("• {task_title} (from *{meeting_title}*)\n"));
        }
        elements.push(serde_json::json!({
            "tag": "div",
            "text": { "content": tasks_md, "tag": "lark_md" }
        }));
    }

    // Footer note
    elements.push(serde_json::json!({ "tag": "hr" }));
    elements.push(serde_json::json!({
        "tag": "note",
        "elements": [{
            "tag": "plain_text",
            "content": "Open Said to join the meeting"
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
        Option<chrono::DateTime<chrono::Utc>>,
    )> = sqlx::query_as("SELECT title, agenda, org_id, scheduled_at FROM meetings WHERE id = $1")
        .bind(meeting_id)
        .fetch_optional(db)
        .await
        .map_err(|e| format!("fetch meeting: {e}"))?;

    let Some((title, agenda_opt, org_id, scheduled_at)) = row else {
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

    // Send card to each participant
    let mut sent = 0i32;
    for (account_id, lark_user_id, _name) in &participants {
        let Some(luid) = lark_user_id else { continue };

        // Tier 2: Carry-forward tasks for this participant
        let open_tasks = get_open_tasks_for_participant(*account_id, org_id, db).await;

        let card = build_meeting_card(
            &title,
            &agenda,
            &participant_names,
            &open_tasks,
            "blue",
            "\u{1F4CB} New Meeting",
        );

        match send_interactive_card(app_id, app_secret, luid, &card).await {
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

            let reminder_card = serde_json::json!({
                "config": { "wide_screen_mode": true },
                "header": {
                    "title": { "content": "\u{23F0} Starting Soon", "tag": "plain_text" },
                    "template": "blue"
                },
                "elements": [{
                    "tag": "div",
                    "text": {
                        "content": format!("**{title}** is starting now.\n\nOpen Said to join."),
                        "tag": "lark_md"
                    }
                }]
            });

            if let Ok(msg_id) =
                send_interactive_card(app_id, app_secret, luid, &reminder_card).await
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
    serde_json::json!({
        "block_type": 12,
        "text": {
            "elements": [{ "text_run": { "content": format!("\u{2022} {text}") } }]
        }
    })
}
