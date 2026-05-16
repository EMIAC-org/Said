//! Background worker that processes pending meeting slots through AI.
//!
//! Every 10 seconds the worker picks the oldest closed-but-unprocessed slot,
//! sends its transcript to the Codex model, parses structured XML output, and
//! persists summaries, tasks, and decisions back to Postgres. Live participants
//! are notified in real time via the [`MeetingHub`].

use std::sync::Arc;

use sqlx::PgPool;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::codex_client;
use crate::meeting_hub::MeetingHub;

// ── Public entry point ──────────────────────────────────────────────────────

/// Spawn the AI worker loop. Call once at startup (after creating the hub).
pub fn start_ai_worker(db: PgPool, hub: Arc<MeetingHub>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
        loop {
            interval.tick().await;
            if let Err(e) = process_next_slot(&db, &hub).await {
                error!("[ai-worker] tick error: {e}");
            }
        }
    });
    info!("[ai-worker] started (10s poll)");
}

// ── Core processing loop ────────────────────────────────────────────────────

/// Pick the next pending slot, run the AI pipeline, and persist results.
/// Returns `Ok(())` even when there is no work — only DB/transport errors
/// propagate.
async fn process_next_slot(db: &PgPool, hub: &Arc<MeetingHub>) -> Result<(), sqlx::Error> {
    // 1. Pick the oldest closed, pending slot.
    let row = sqlx::query_as::<_, SlotRow>(
        "SELECT id, meeting_id, slot_index, start_ms, end_ms, chunk_count, word_count
         FROM meeting_slots
         WHERE ai_status = 'pending' AND end_ms IS NOT NULL
         ORDER BY created_at ASC
         LIMIT 1",
    )
    .fetch_optional(db)
    .await?;

    let slot = match row {
        Some(s) => s,
        None => return Ok(()), // nothing to do
    };

    info!(
        "[ai-worker] processing slot {} for meeting {} ({} chunks, {} words)",
        slot.slot_index, slot.meeting_id, slot.chunk_count, slot.word_count
    );

    // 2. Mark as processing.
    sqlx::query("UPDATE meeting_slots SET ai_status = 'processing' WHERE id = $1")
        .bind(slot.id)
        .execute(db)
        .await?;

    // Run the pipeline — any failure sets ai_status = 'failed'.
    match run_pipeline(&slot, db, hub).await {
        Ok(()) => {
            info!("[ai-worker] slot {} done", slot.slot_index);
        }
        Err(e) => {
            error!("[ai-worker] slot {} failed: {e}", slot.slot_index);
            let _ = sqlx::query("UPDATE meeting_slots SET ai_status = 'failed' WHERE id = $1")
                .bind(slot.id)
                .execute(db)
                .await;
        }
    }

    Ok(())
}

// ── Pipeline ────────────────────────────────────────────────────────────────

async fn run_pipeline(
    slot: &SlotRow,
    db: &PgPool,
    hub: &Arc<MeetingHub>,
) -> Result<(), PipelineError> {
    // 3a. Fetch the org's OpenAI access token.
    let access_token: Option<String> = sqlx::query_scalar(
        "SELECT o.openai_access_token
         FROM meetings m
         JOIN orgs o ON o.id = m.org_id
         WHERE m.id = $1",
    )
    .bind(slot.meeting_id)
    .fetch_optional(db)
    .await
    .map_err(PipelineError::Db)?
    .flatten();

    let access_token = match access_token {
        Some(t) if !t.is_empty() => t,
        _ => {
            warn!(
                "[ai-worker] no OpenAI access token for meeting {} — skipping slot {}",
                slot.meeting_id, slot.slot_index
            );
            let _ = sqlx::query("UPDATE meeting_slots SET ai_status = 'skipped' WHERE id = $1")
                .bind(slot.id)
                .execute(db)
                .await;
            return Ok(());
        }
    };

    // 3b. Previous summary (if any).
    let previous_summary: Option<String> =
        sqlx::query_scalar("SELECT summary_text FROM meeting_summaries WHERE meeting_id = $1")
            .bind(slot.meeting_id)
            .fetch_optional(db)
            .await
            .map_err(PipelineError::Db)?;

    // 3c. Transcript chunks within this slot's time range.
    let chunks = sqlx::query_as::<_, ChunkRow>(
        "SELECT tc.speaker_id, tc.text,
                COALESCE(om.lark_name, a.email) AS speaker_name
         FROM transcript_chunks tc
         JOIN accounts a ON a.id = tc.speaker_id
         LEFT JOIN org_members om ON om.account_id = tc.speaker_id
              AND om.org_id = (SELECT org_id FROM meetings WHERE id = $1)
         WHERE tc.meeting_id = $1
           AND tc.timestamp_ms >= $2
           AND tc.timestamp_ms <= $3
         ORDER BY tc.chunk_index ASC",
    )
    .bind(slot.meeting_id)
    .bind(slot.start_ms)
    .bind(slot.end_ms)
    .fetch_all(db)
    .await
    .map_err(PipelineError::Db)?;

    if chunks.is_empty() {
        warn!(
            "[ai-worker] slot {} has no transcript chunks — marking done",
            slot.slot_index
        );
        sqlx::query(
            "UPDATE meeting_slots SET ai_status = 'done', summary_delta = '', tasks_detected = 0, decisions_detected = 0 WHERE id = $1"
        )
        .bind(slot.id)
        .execute(db)
        .await
        .map_err(PipelineError::Db)?;
        return Ok(());
    }

    // 4. Build the prompt.
    let word_count: usize = chunks
        .iter()
        .map(|c| c.text.split_whitespace().count())
        .sum();

    let mut transcript_lines = String::new();
    for c in &chunks {
        transcript_lines.push_str(&format!("[{}]: {}\n", c.speaker_name, c.text));
    }

    let prev_summary_text = previous_summary
        .as_deref()
        .unwrap_or("No previous summary — this is the start of the meeting.");

    let instructions = SYSTEM_INSTRUCTIONS;

    let user_input = format!(
        "Previous summary: {prev_summary_text}\n\n\
         Transcript (Slot {slot_index}, {chunk_count} chunks, {wc} words):\n\
         {transcript_lines}",
        slot_index = slot.slot_index,
        chunk_count = chunks.len(),
        wc = word_count,
    );

    // 5. Call Codex.
    let response = codex_client::call_codex(&access_token, MODEL, instructions, &user_input)
        .await
        .map_err(|e| PipelineError::Codex(format!("{e}")))?;

    // 6. Parse the XML response.
    let intel = parse_meeting_intelligence(&response.text);

    // 7. Store results.

    // 7a. Upsert meeting summary.
    sqlx::query(
        "INSERT INTO meeting_summaries (meeting_id, summary_text, updated_at)
         VALUES ($1, $2, now())
         ON CONFLICT (meeting_id) DO UPDATE SET summary_text = $2, updated_at = now()",
    )
    .bind(slot.meeting_id)
    .bind(&intel.summary)
    .execute(db)
    .await
    .map_err(PipelineError::Db)?;

    // 7b. Resolve org_id for assignee matching.
    let org_id: Option<Uuid> = sqlx::query_scalar("SELECT org_id FROM meetings WHERE id = $1")
        .bind(slot.meeting_id)
        .fetch_optional(db)
        .await
        .map_err(PipelineError::Db)?;

    // 7c. Insert tasks.
    let mut task_count = 0i32;
    for task in &intel.tasks {
        let resolved = if let Some(oid) = org_id {
            resolve_assignee(&task.assignee, oid, db).await
        } else {
            None
        };

        let (assignee_id, assignee_name) = match &resolved {
            Some((aid, aname)) => (Some(*aid), Some(aname.clone())),
            None => (None, None),
        };

        let task_id: Uuid = sqlx::query_scalar(
            "INSERT INTO meeting_tasks (meeting_id, title, assignee_id, status)
             VALUES ($1, $2, $3, 'draft')
             RETURNING id",
        )
        .bind(slot.meeting_id)
        .bind(&task.title)
        .bind(assignee_id)
        .fetch_one(db)
        .await
        .map_err(PipelineError::Db)?;

        task_count += 1;

        // 8a. Notify live participants about the task.
        hub.notify_task(
            slot.meeting_id,
            task_id,
            task.title.clone(),
            assignee_id,
            assignee_name,
        )
        .await;
    }

    // 7d. Insert decisions.
    let mut decision_count = 0i32;
    for decision in &intel.decisions {
        sqlx::query("INSERT INTO meeting_decisions (meeting_id, text) VALUES ($1, $2)")
            .bind(slot.meeting_id)
            .bind(decision)
            .execute(db)
            .await
            .map_err(PipelineError::Db)?;

        decision_count += 1;
    }

    // 7e. Update slot as done.
    sqlx::query(
        "UPDATE meeting_slots
         SET ai_status = 'done',
             summary_delta = $1,
             tasks_detected = $2,
             decisions_detected = $3
         WHERE id = $4",
    )
    .bind(&intel.summary)
    .bind(task_count)
    .bind(decision_count)
    .bind(slot.id)
    .execute(db)
    .await
    .map_err(PipelineError::Db)?;

    // 8b. Broadcast updated summary to live participants.
    hub.broadcast_summary(slot.meeting_id, intel.summary).await;

    info!(
        "[ai-worker] slot {} → {} tasks, {} decisions",
        slot.slot_index, task_count, decision_count
    );

    Ok(())
}

// ── XML parsing helpers ─────────────────────────────────────────────────────

struct ParsedIntelligence {
    summary: String,
    tasks: Vec<ParsedTask>,
    decisions: Vec<String>,
}

struct ParsedTask {
    title: String,
    assignee: String,
}

/// Extract structured intelligence from the Codex XML response.
///
/// Uses simple string-based parsing — no XML crate required.
fn parse_meeting_intelligence(xml: &str) -> ParsedIntelligence {
    let summary = extract_tag(xml, "summary").unwrap_or_default();

    let tasks = extract_all_tags(xml, "task")
        .into_iter()
        .filter_map(|task_xml| {
            let title = extract_tag(&task_xml, "title")?;
            let assignee = extract_tag(&task_xml, "assignee").unwrap_or_default();
            if title.is_empty() {
                return None;
            }
            Some(ParsedTask { title, assignee })
        })
        .collect();

    let decisions = extract_all_tags(xml, "decision")
        .into_iter()
        .filter(|d| !d.is_empty())
        .collect();

    ParsedIntelligence {
        summary,
        tasks,
        decisions,
    }
}

/// Extract the text content between `<tag>` and `</tag>`.
fn extract_tag(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(xml[start..end].trim().to_string())
}

/// Extract all occurrences of `<tag>...</tag>`.
fn extract_all_tags(xml: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut results = Vec::new();
    let mut search_from = 0;

    while let Some(start_offset) = xml[search_from..].find(&open) {
        let content_start = search_from + start_offset + open.len();
        if let Some(end_offset) = xml[content_start..].find(&close) {
            let content_end = content_start + end_offset;
            results.push(xml[content_start..content_end].trim().to_string());
            search_from = content_end + close.len();
        } else {
            break;
        }
    }

    results
}

// ── Assignee resolution ─────────────────────────────────────────────────────

/// Fuzzy-match an assignee name against org members. Returns (account_id,
/// lark_name) on success. Matching is case-insensitive and uses `contains`
/// semantics (e.g. "Rahul" matches "Rahul Kumar").
async fn resolve_assignee(name: &str, org_id: Uuid, db: &PgPool) -> Option<(Uuid, String)> {
    if name.is_empty() {
        return None;
    }

    let members = sqlx::query_as::<_, OrgMemberRow>(
        "SELECT account_id, lark_name FROM org_members WHERE org_id = $1 AND lark_name IS NOT NULL",
    )
    .bind(org_id)
    .fetch_all(db)
    .await
    .ok()?;

    let name_lower = name.to_lowercase();

    members.into_iter().find_map(|m| {
        let lark_lower = m.lark_name.to_lowercase();
        if lark_lower.contains(&name_lower) || name_lower.contains(&lark_lower) {
            Some((m.account_id, m.lark_name))
        } else {
            None
        }
    })
}

// ── Row types ───────────────────────────────────────────────────────────────

#[derive(sqlx::FromRow)]
struct SlotRow {
    id: Uuid,
    meeting_id: Uuid,
    slot_index: i32,
    start_ms: i64,
    end_ms: i64,
    chunk_count: i32,
    word_count: i32,
}

#[derive(sqlx::FromRow)]
struct ChunkRow {
    #[allow(dead_code)]
    speaker_id: Uuid,
    text: String,
    speaker_name: String,
}

#[derive(sqlx::FromRow)]
struct OrgMemberRow {
    account_id: Uuid,
    lark_name: String,
}

// ── Error type ──────────────────────────────────────────────────────────────

#[derive(Debug)]
enum PipelineError {
    Db(sqlx::Error),
    Codex(String),
}

impl std::fmt::Display for PipelineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Db(e) => write!(f, "database error: {e}"),
            Self::Codex(e) => write!(f, "codex error: {e}"),
        }
    }
}

// ── Constants ───────────────────────────────────────────────────────────────

const MODEL: &str = "gpt-5.4-mini";

const SYSTEM_INSTRUCTIONS: &str = "\
You are a meeting intelligence assistant for an enterprise team. \
Analyze the new transcript segment and produce structured intelligence.

Rules:
- Update the running summary to incorporate the new discussion naturally
- Extract specific, actionable tasks with assignees when mentioned by name
- Extract concrete decisions (not vague statements)
- If no new tasks or decisions, leave those sections empty
- Keep the summary concise (3-5 sentences max)
- Use the speaker names as given

Respond ONLY with this XML structure, no other text:
<meeting_intelligence>
  <summary>Updated meeting summary incorporating all discussion so far</summary>
  <tasks>
    <task><title>Specific task description</title><assignee>Person name</assignee></task>
  </tasks>
  <decisions>
    <decision>Concrete decision that was made</decision>
  </decisions>
</meeting_intelligence>";
