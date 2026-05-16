//! Meeting routes:
//!   POST /v1/meetings               — create a meeting
//!   GET  /v1/meetings               — list meetings for user's org (?status=live|ended|scheduled)
//!   GET  /v1/meetings/:id           — get meeting detail with participants
//!   POST /v1/meetings/:id/start     — set status to 'live'
//!   POST /v1/meetings/:id/end       — set status to 'ended'
//!   POST /v1/meetings/:id/push-tasks — push draft tasks to Lark

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{AppState, auth::AuthUser};

// ── Request / response types ────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CreateMeetingBody {
    pub title: String,
    pub agenda: Option<String>,
    /// Account IDs of participants to invite (creator is added automatically).
    pub participant_ids: Vec<Uuid>,
    /// Optional scheduled start time (used for calendar events + reminders).
    pub scheduled_at: Option<DateTime<Utc>>,
}

#[derive(Deserialize)]
pub struct ListMeetingsQuery {
    /// Filter by status: "scheduled", "live", or "ended". If omitted, returns all.
    pub status: Option<String>,
}

#[derive(Deserialize)]
pub struct PushTasksBody {
    pub task_ids: Vec<Uuid>,
}

#[derive(Serialize)]
pub struct MeetingInfo {
    pub id: Uuid,
    pub org_id: Uuid,
    pub title: String,
    pub agenda: Option<String>,
    pub status: String,
    pub created_by: Uuid,
    pub started_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Serialize)]
pub struct ParticipantInfo {
    pub id: Uuid,
    pub account_id: Uuid,
    pub status: String,
    pub joined_at: Option<DateTime<Utc>>,
    pub left_at: Option<DateTime<Utc>>,
    pub disconnect_count: i32,
}

// ── POST /v1/meetings ───────────────────────────────────────────────────────

pub async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<CreateMeetingBody>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<Value>)> {
    let title = body.title.trim().to_string();
    if title.is_empty() {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"error": "title is required"})),
        ));
    }

    // Resolve caller's org and role
    let (org_id, caller_role) = resolve_org_and_role(&state, user.account_id).await?;

    // Check if caller's role permits creating meetings
    let creator_roles: Value =
        sqlx::query_scalar("SELECT meeting_creator_roles FROM orgs WHERE id = $1")
            .bind(org_id)
            .fetch_one(&state.db)
            .await
            .map_err(db_err)?;

    let allowed = creator_roles
        .as_array()
        .map(|arr| {
            arr.iter().any(|v| {
                let Some(role) = v.as_str() else {
                    return false;
                };
                role.eq_ignore_ascii_case(&caller_role)
                    || (caller_role.eq_ignore_ascii_case("admin")
                        && role.eq_ignore_ascii_case("COMPANY_ADMIN"))
            })
        })
        .unwrap_or(false);

    if !allowed {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({"error": "your role does not permit creating meetings"})),
        ));
    }

    // Insert meeting
    let meeting_id: Uuid = sqlx::query_scalar(
        "INSERT INTO meetings (org_id, title, agenda, scheduled_at, created_by)
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(org_id)
    .bind(&title)
    .bind(&body.agenda)
    .bind(body.scheduled_at)
    .bind(user.account_id)
    .fetch_one(&state.db)
    .await
    .map_err(db_err)?;

    // Add creator as participant (status = 'invited')
    sqlx::query(
        "INSERT INTO meeting_participants (meeting_id, account_id)
         VALUES ($1, $2)
         ON CONFLICT (meeting_id, account_id) DO NOTHING",
    )
    .bind(meeting_id)
    .bind(user.account_id)
    .execute(&state.db)
    .await
    .map_err(db_err)?;

    // Add other participants
    for &pid in &body.participant_ids {
        if pid == user.account_id {
            continue; // already added
        }
        sqlx::query(
            "INSERT INTO meeting_participants (meeting_id, account_id)
             VALUES ($1, $2)
             ON CONFLICT (meeting_id, account_id) DO NOTHING",
        )
        .bind(meeting_id)
        .bind(pid)
        .execute(&state.db)
        .await
        .map_err(db_err)?;
    }

    // Fire-and-forget Lark pre-meeting notification (Tier 1 + 2)
    {
        let lark_cfg = state.lark.clone();
        let db_clone = state.db.clone();
        let mid = meeting_id;
        tokio::spawn(async move {
            if lark_cfg.app_id.is_empty() {
                return;
            }
            match crate::lark_sync::send_meeting_notification(
                &lark_cfg.app_id,
                &lark_cfg.app_secret,
                mid,
                &db_clone,
            )
            .await
            {
                Ok(count) => tracing::info!("[lark-notify] sent {count} cards for meeting {mid}"),
                Err(e) => tracing::warn!("[lark-notify] failed for meeting {mid}: {e}"),
            }
        });
    }

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "meeting": {
                "id":           meeting_id,
                "org_id":       org_id,
                "title":        title,
                "agenda":       body.agenda,
                "scheduled_at": body.scheduled_at,
                "status":       "scheduled",
                "created_by":   user.account_id,
            }
        })),
    ))
}

// ── GET /v1/meetings ────────────────────────────────────────────────────────

pub async fn list(
    State(state): State<AppState>,
    user: AuthUser,
    Query(query): Query<ListMeetingsQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let org_id = resolve_org(&state, user.account_id).await?;

    let meetings: Vec<(
        Uuid,
        String,
        Option<String>,
        String,
        Uuid,
        Option<DateTime<Utc>>,
        Option<DateTime<Utc>>,
        DateTime<Utc>,
    )> = if let Some(status) = &query.status {
        sqlx::query_as(
            "SELECT id, title, agenda, status, created_by, started_at, ended_at, created_at
                   FROM meetings
                  WHERE org_id = $1 AND status = $2
                  ORDER BY created_at DESC",
        )
        .bind(org_id)
        .bind(status)
        .fetch_all(&state.db)
        .await
        .map_err(db_err)?
    } else {
        sqlx::query_as(
            "SELECT id, title, agenda, status, created_by, started_at, ended_at, created_at
                   FROM meetings
                  WHERE org_id = $1
                  ORDER BY created_at DESC",
        )
        .bind(org_id)
        .fetch_all(&state.db)
        .await
        .map_err(db_err)?
    };

    let meetings_json: Vec<Value> = meetings
        .into_iter()
        .map(
            |(id, title, agenda, status, created_by, started_at, ended_at, created_at)| {
                json!({
                    "id":         id,
                    "title":      title,
                    "agenda":     agenda,
                    "status":     status,
                    "created_by": created_by,
                    "started_at": started_at,
                    "ended_at":   ended_at,
                    "created_at": created_at,
                })
            },
        )
        .collect();

    Ok(Json(json!({ "meetings": meetings_json })))
}

// ── GET /v1/meetings/:id ────────────────────────────────────────────────────

pub async fn detail(
    State(state): State<AppState>,
    user: AuthUser,
    Path(meeting_id): Path<Uuid>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let org_id = resolve_org(&state, user.account_id).await?;

    // Fetch meeting (scoped to caller's org)
    let meeting: Option<(
        Uuid,
        String,
        Option<String>,
        String,
        Uuid,
        Option<DateTime<Utc>>,
        Option<DateTime<Utc>>,
        DateTime<Utc>,
    )> = sqlx::query_as(
        "SELECT id, title, agenda, status, created_by, started_at, ended_at, created_at
               FROM meetings
              WHERE id = $1 AND org_id = $2",
    )
    .bind(meeting_id)
    .bind(org_id)
    .fetch_optional(&state.db)
    .await
    .map_err(db_err)?;

    let Some((id, title, agenda, status, created_by, started_at, ended_at, created_at)) = meeting
    else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error": "meeting not found"})),
        ));
    };

    // Fetch participants (including disconnect_count)
    let participants: Vec<(
        Uuid,
        Uuid,
        String,
        Option<DateTime<Utc>>,
        Option<DateTime<Utc>>,
        i32,
    )> = sqlx::query_as(
        "SELECT id, account_id, status, joined_at, left_at, disconnect_count
               FROM meeting_participants
              WHERE meeting_id = $1
              ORDER BY joined_at ASC NULLS LAST",
    )
    .bind(meeting_id)
    .fetch_all(&state.db)
    .await
    .map_err(db_err)?;

    let participants_json: Vec<Value> = participants
        .into_iter()
        .map(
            |(pid, account_id, pstatus, joined_at, left_at, disconnect_count)| {
                json!({
                    "id":               pid,
                    "account_id":       account_id,
                    "status":           pstatus,
                    "joined_at":        joined_at,
                    "left_at":          left_at,
                    "disconnect_count": disconnect_count,
                })
            },
        )
        .collect();

    // Fetch latest summary
    let summary: Option<String> = sqlx::query_scalar(
        "SELECT summary_text FROM meeting_summaries WHERE meeting_id = $1
         ORDER BY updated_at DESC LIMIT 1",
    )
    .bind(meeting_id)
    .fetch_optional(&state.db)
    .await
    .map_err(db_err)?;

    // Fetch tasks with assignee names
    let tasks: Vec<(Uuid, String, Option<String>, String, Option<String>)> = sqlx::query_as(
        "SELECT mt.id, mt.title,
                COALESCE(NULLIF(om.lark_name, ''), split_part(a.email, '@', 1)),
                mt.status, mt.lark_task_id
           FROM meeting_tasks mt
           LEFT JOIN accounts a ON a.id = mt.assignee_id
           LEFT JOIN org_members om ON om.account_id = mt.assignee_id AND om.org_id = $2
          WHERE mt.meeting_id = $1
          ORDER BY mt.detected_at ASC",
    )
    .bind(meeting_id)
    .bind(org_id)
    .fetch_all(&state.db)
    .await
    .map_err(db_err)?;

    let tasks_json: Vec<Value> = tasks
        .into_iter()
        .map(|(tid, title, assignee, tstatus, lark_id)| {
            json!({
                "id":           tid,
                "title":        title,
                "assignee":     assignee,
                "status":       tstatus,
                "lark_task_id": lark_id,
            })
        })
        .collect();

    // Fetch decisions
    let decisions: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT id, text FROM meeting_decisions WHERE meeting_id = $1 ORDER BY detected_at ASC",
    )
    .bind(meeting_id)
    .fetch_all(&state.db)
    .await
    .map_err(db_err)?;

    let decisions_json: Vec<Value> = decisions
        .into_iter()
        .map(|(did, text)| json!({"id": did, "text": text}))
        .collect();

    // Fetch transcript with speaker names
    let transcript: Vec<(String, Option<String>, String, i64, i32)> = sqlx::query_as(
        "SELECT tc.speaker_id::text,
                COALESCE(NULLIF(om.lark_name, ''), split_part(a.email, '@', 1)),
                tc.text, tc.timestamp_ms, tc.chunk_index
           FROM transcript_chunks tc
           JOIN accounts a ON a.id = tc.speaker_id
           LEFT JOIN org_members om ON om.account_id = tc.speaker_id AND om.org_id = $2
          WHERE tc.meeting_id = $1
          ORDER BY tc.chunk_index ASC",
    )
    .bind(meeting_id)
    .bind(org_id)
    .fetch_all(&state.db)
    .await
    .map_err(db_err)?;

    let transcript_json: Vec<Value> = transcript
        .into_iter()
        .map(|(speaker_id, speaker_name, text, ts, idx)| {
            json!({
                "speaker_id":   speaker_id,
                "speaker_name": speaker_name,
                "text":         text,
                "timestamp_ms": ts,
                "chunk_index":  idx,
            })
        })
        .collect();

    Ok(Json(json!({
        "meeting": {
            "id":           id,
            "org_id":       org_id,
            "title":        title,
            "agenda":       agenda,
            "status":       status,
            "created_by":   created_by,
            "started_at":   started_at,
            "ended_at":     ended_at,
            "created_at":   created_at,
        },
        "participants": participants_json,
        "summary":      summary,
        "tasks":        tasks_json,
        "decisions":    decisions_json,
        "transcript":   transcript_json,
    })))
}

// ── POST /v1/meetings/:id/start ─────────────────────────────────────────────

pub async fn start(
    State(state): State<AppState>,
    user: AuthUser,
    Path(meeting_id): Path<Uuid>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let org_id = resolve_org(&state, user.account_id).await?;

    let now = Utc::now();
    let rows_affected = sqlx::query(
        "UPDATE meetings
            SET status = 'live', started_at = $1
          WHERE id = $2 AND org_id = $3 AND status = 'scheduled'",
    )
    .bind(now)
    .bind(meeting_id)
    .bind(org_id)
    .execute(&state.db)
    .await
    .map_err(db_err)?
    .rows_affected();

    if rows_affected == 0 {
        return Err((
            StatusCode::CONFLICT,
            Json(json!({"error": "meeting not found or not in 'scheduled' status"})),
        ));
    }

    Ok(Json(json!({
        "meeting": {
            "id":         meeting_id,
            "status":     "live",
            "started_at": now,
        }
    })))
}

// ── POST /v1/meetings/:id/end ───────────────────────────────────────────────

pub async fn end(
    State(state): State<AppState>,
    user: AuthUser,
    Path(meeting_id): Path<Uuid>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let org_id = resolve_org(&state, user.account_id).await?;

    let now = Utc::now();
    let rows_affected = sqlx::query(
        "UPDATE meetings
            SET status = 'ended', ended_at = $1
          WHERE id = $2 AND org_id = $3 AND status = 'live' AND created_by = $4",
    )
    .bind(now)
    .bind(meeting_id)
    .bind(org_id)
    .bind(user.account_id)
    .execute(&state.db)
    .await
    .map_err(db_err)?
    .rows_affected();

    if rows_affected == 0 {
        return Err((
            StatusCode::CONFLICT,
            Json(json!({"error": "meeting not found, not live, or not owned by caller"})),
        ));
    }

    // Mark all non-left participants as 'left'
    let participants_ended: u64 = sqlx::query(
        "UPDATE meeting_participants
            SET status = 'left', left_at = $1
          WHERE meeting_id = $2 AND status != 'left'",
    )
    .bind(now)
    .bind(meeting_id)
    .execute(&state.db)
    .await
    .map_err(db_err)?
    .rows_affected();

    // Broadcast meeting end to all connected WebSocket clients + close open slot
    state.hub.broadcast_meeting_end(meeting_id, &state.db).await;

    Ok(Json(json!({
        "meeting": {
            "id":                meeting_id,
            "status":            "ended",
            "ended_at":          now,
            "participants_ended": participants_ended,
        }
    })))
}

// ── POST /v1/meetings/:id/push-tasks ───────────────────────────────────────

pub async fn push_tasks(
    State(state): State<AppState>,
    user: AuthUser,
    Path(meeting_id): Path<Uuid>,
    Json(body): Json<PushTasksBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let org_id = resolve_org(&state, user.account_id).await?;

    // Verify meeting belongs to this org
    let meeting_exists: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM meetings WHERE id = $1 AND org_id = $2")
            .bind(meeting_id)
            .bind(org_id)
            .fetch_optional(&state.db)
            .await
            .map_err(db_err)?;

    if meeting_exists.is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error": "meeting not found"})),
        ));
    }

    let mut pushed: i64 = 0;
    let mut skipped: i64 = 0;

    for task_id in &body.task_ids {
        // Fetch task: must belong to this meeting and not already synced
        let task: Option<(Uuid, String, Option<Uuid>, Option<String>)> = sqlx::query_as(
            "SELECT id, title, assignee_id, lark_task_id
               FROM meeting_tasks
              WHERE id = $1 AND meeting_id = $2",
        )
        .bind(task_id)
        .bind(meeting_id)
        .fetch_optional(&state.db)
        .await
        .map_err(db_err)?;

        let Some((_tid, title, assignee_id, lark_task_id)) = task else {
            // Task doesn't belong to this meeting — skip silently
            skipped += 1;
            continue;
        };

        if lark_task_id.is_some() {
            // Already synced
            skipped += 1;
            continue;
        }

        // Look up assignee's lark_user_id via org_members
        let assignee_lark_user_id: Option<String> = if let Some(aid) = assignee_id {
            sqlx::query_scalar(
                "SELECT lark_user_id FROM org_members WHERE account_id = $1 AND org_id = $2",
            )
            .bind(aid)
            .bind(org_id)
            .fetch_optional(&state.db)
            .await
            .map_err(db_err)?
            .flatten()
        } else {
            None
        };

        // Call Lark to create the task
        let lark_cfg = &state.lark;
        match crate::lark_sync::create_lark_task(
            &lark_cfg.app_id,
            &lark_cfg.app_secret,
            &title,
            None,
            assignee_lark_user_id.as_deref(),
        )
        .await
        {
            Ok(guid) => {
                // Update the task: set lark_task_id and status = 'synced'
                sqlx::query(
                    "UPDATE meeting_tasks SET lark_task_id = $1, status = 'synced' WHERE id = $2",
                )
                .bind(&guid)
                .bind(task_id)
                .execute(&state.db)
                .await
                .map_err(db_err)?;

                pushed += 1;
            }
            Err(e) => {
                tracing::warn!("push_tasks: failed to create lark task {task_id}: {e}");
                // Count as skipped rather than failing the whole request
                skipped += 1;
            }
        }
    }

    Ok(Json(json!({ "pushed": pushed, "skipped": skipped })))
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Resolve the caller's org_id AND role from org_members, or return 403.
async fn resolve_org_and_role(
    state: &AppState,
    account_id: Uuid,
) -> Result<(Uuid, String), (StatusCode, Json<Value>)> {
    let row: Option<(Uuid, String)> =
        sqlx::query_as("SELECT org_id, role FROM org_members WHERE account_id = $1 LIMIT 1")
            .bind(account_id)
            .fetch_optional(&state.db)
            .await
            .map_err(db_err)?;

    row.ok_or_else(|| {
        (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "you must belong to an org"})),
        )
    })
}

/// Resolve the caller's org_id from org_members, or return 403.
async fn resolve_org(
    state: &AppState,
    account_id: Uuid,
) -> Result<Uuid, (StatusCode, Json<Value>)> {
    let (org_id, _role) = resolve_org_and_role(state, account_id).await?;
    Ok(org_id)
}

fn db_err(_e: sqlx::Error) -> (StatusCode, Json<Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": "database error"})),
    )
}
