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
    http::{HeaderMap, StatusCode},
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Instant;

use crate::{AppState, auth::AuthUser, costs, tenant};

// ── Request / response types ────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CreateMeetingBody {
    pub title: String,
    pub agenda: Option<String>,
    /// Account IDs of participants to invite (creator is added automatically).
    pub participant_ids: Vec<Uuid>,
    /// Optional scheduled start time (used for calendar events + reminders).
    pub scheduled_at: Option<DateTime<Utc>>,
    /// Scheduled meeting duration in minutes. Defaults to 30.
    pub duration_minutes: Option<i32>,
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
    pub scheduled_at: Option<DateTime<Utc>>,
    pub duration_minutes: i32,
    pub lark_calendar_id: Option<String>,
    pub lark_event_id: Option<String>,
    pub lark_event_status: String,
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
    headers: HeaderMap,
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
    let duration_minutes = body.duration_minutes.unwrap_or(30);
    if !(1..=1440).contains(&duration_minutes) {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"error": "duration_minutes must be between 1 and 1440"})),
        ));
    }

    let (org_id, caller_role) = tenant::require_active_org_role(&state, &user, &headers).await?;

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

    if body.participant_ids.is_empty() {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"error": "at least one participant must be invited"})),
        ));
    }

    // Insert meeting
    let meeting_id: Uuid = sqlx::query_scalar(
        "INSERT INTO meetings (org_id, title, agenda, scheduled_at, duration_minutes, created_by)
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
    )
    .bind(org_id)
    .bind(&title)
    .bind(&body.agenda)
    .bind(body.scheduled_at)
    .bind(duration_minutes)
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
                "duration_minutes": duration_minutes,
                "lark_event_status": "pending",
                "status":       "scheduled",
                "created_by":   user.account_id,
            }
        })),
    ))
}

// ── GET /v1/meetings ────────────────────────────────────────────────────────

/// SQL fragment (table alias `m`) matching an "abandoned empty" meeting: it has
/// no streamed transcript and no summary, AND it either ended within a minute
/// (recorded nothing) or is stuck `live` for many hours (the app was force-quit
/// before `/end`). A REAL meeting streams `transcript_chunks` to the hub live, so
/// it can never match — that, plus the duration guard, makes this safe even if a
/// real meeting's WebSocket dropped. Empty meetings pile up because "New Meeting"
/// creates the cloud record eagerly (before any audio); this is the server-side,
/// creator-independent cleanup that the previous device-local cleanup could not do.
const EMPTY_MEETING_PREDICATE: &str = "
    NOT EXISTS (SELECT 1 FROM transcript_chunks t WHERE t.meeting_id = m.id)
    AND NOT EXISTS (SELECT 1 FROM meeting_summaries s WHERE s.meeting_id = m.id)
    AND (
        (m.status = 'ended' AND COALESCE(m.ended_at - m.started_at, interval '0') < interval '60 seconds')
        OR (m.status = 'live' AND m.started_at < now() - interval '12 hours')
    )";

/// Per-org throttle so the reap runs at most once every couple of minutes no
/// matter how often the meetings list is polled. In-memory is fine: a missed
/// reap just happens on the next list call.
static REAP_LAST: LazyLock<Mutex<HashMap<Uuid, Instant>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Delete abandoned empty meetings for an org. A 30-minute grace lets the
/// desktop's own per-session cleanup (and any late transcript delivery) settle
/// first. Children cascade via `ON DELETE CASCADE`.
async fn reap_empty_meetings(db: &crate::store::Db, org_id: Uuid) -> Result<u64, sqlx::Error> {
    let sql = format!(
        "DELETE FROM meetings m
          WHERE m.org_id = $1
            AND m.created_at < now() - interval '30 minutes'
            AND ({EMPTY_MEETING_PREDICATE})"
    );
    let affected = sqlx::query(&sql)
        .bind(org_id)
        .execute(db)
        .await?
        .rows_affected();
    if affected > 0 {
        tracing::info!(%org_id, reaped = affected, "[meetings] reaped abandoned empty meetings");
    }
    Ok(affected)
}

/// Fire-and-forget, throttled reap. Never blocks the request — the list query's
/// own filter already hides empties immediately; this just keeps the table tidy.
fn maybe_reap_empty_meetings(state: &AppState, org_id: Uuid) {
    {
        let mut last = match REAP_LAST.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let now = Instant::now();
        if let Some(prev) = last.get(&org_id) {
            if now.duration_since(*prev) < std::time::Duration::from_secs(120) {
                return;
            }
        }
        last.insert(org_id, now);
    }
    let db = state.db.clone();
    tokio::spawn(async move {
        if let Err(e) = reap_empty_meetings(&db, org_id).await {
            tracing::warn!(%org_id, error = %e, "[meetings] empty-meeting reap failed");
        }
    });
}

pub async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Query(query): Query<ListMeetingsQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let (_, org_id) = tenant::require_active_org(&state, &user, &headers).await?;

    // Self-healing cleanup: drop abandoned empty meetings for this org (throttled,
    // off the request path). The query filter below also hides them immediately,
    // so even a brand-new user inherits a clean list instead of the org's backlog.
    maybe_reap_empty_meetings(&state, org_id);

    let meetings: Vec<(
        Uuid,
        String,
        Option<String>,
        String,
        Uuid,
        Option<DateTime<Utc>>,
        Option<DateTime<Utc>>,
        DateTime<Utc>,
        Option<DateTime<Utc>>,
        i32,
        Option<String>,
        Option<String>,
        String,
    )> = if let Some(status) = &query.status {
        sqlx::query_as(&format!(
            "SELECT id, title, agenda, status, created_by, started_at, ended_at, created_at,
                    scheduled_at, duration_minutes, lark_calendar_id, lark_event_id, lark_event_status
                   FROM meetings m
                  WHERE m.org_id = $1 AND m.status = $2 AND NOT ({EMPTY_MEETING_PREDICATE})
                  ORDER BY m.created_at DESC"
        ))
        .bind(org_id)
        .bind(status)
        .fetch_all(&state.db)
        .await
        .map_err(db_err)?
    } else {
        sqlx::query_as(&format!(
            "SELECT id, title, agenda, status, created_by, started_at, ended_at, created_at,
                    scheduled_at, duration_minutes, lark_calendar_id, lark_event_id, lark_event_status
                   FROM meetings m
                  WHERE m.org_id = $1 AND NOT ({EMPTY_MEETING_PREDICATE})
                  ORDER BY m.created_at DESC"
        ))
        .bind(org_id)
        .fetch_all(&state.db)
        .await
        .map_err(db_err)?
    };

    let meetings_json: Vec<Value> = meetings
        .into_iter()
        .map(
            |(
                id,
                title,
                agenda,
                status,
                created_by,
                started_at,
                ended_at,
                created_at,
                scheduled_at,
                duration_minutes,
                lark_calendar_id,
                lark_event_id,
                lark_event_status,
            )| {
                json!({
                    "id":                id,
                    "title":             title,
                    "agenda":            agenda,
                    "status":            status,
                    "created_by":        created_by,
                    "started_at":        started_at,
                    "ended_at":          ended_at,
                    "created_at":        created_at,
                    "scheduled_at":      scheduled_at,
                    "duration_minutes":  duration_minutes,
                    "lark_calendar_id":  lark_calendar_id,
                    "lark_event_id":     lark_event_id,
                    "lark_event_status": lark_event_status,
                })
            },
        )
        .collect();

    Ok(Json(json!({ "meetings": meetings_json })))
}

// ── GET /v1/meetings/:id ────────────────────────────────────────────────────

pub async fn detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Path(meeting_id): Path<Uuid>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let (_, org_id) = tenant::require_active_org(&state, &user, &headers).await?;

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
        Option<DateTime<Utc>>,
        i32,
        Option<String>,
        Option<String>,
        String,
    )> = sqlx::query_as(
        "SELECT id, title, agenda, status, created_by, started_at, ended_at, created_at,
                scheduled_at, duration_minutes, lark_calendar_id, lark_event_id, lark_event_status
               FROM meetings
              WHERE id = $1 AND org_id = $2",
    )
    .bind(meeting_id)
    .bind(org_id)
    .fetch_optional(&state.db)
    .await
    .map_err(db_err)?;

    let Some((
        id,
        title,
        agenda,
        status,
        created_by,
        started_at,
        ended_at,
        created_at,
        scheduled_at,
        duration_minutes,
        lark_calendar_id,
        lark_event_id,
        lark_event_status,
    )) = meeting
    else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error": "meeting not found"})),
        ));
    };

    // Fetch participants with resolved display names.
    // Guest accounts have app-owned emails — strip the trailing -{uuid32} suffix.
    let participants: Vec<(
        Uuid,
        Uuid,
        String,
        Option<DateTime<Utc>>,
        Option<DateTime<Utc>>,
        i32,
        String,
    )> = sqlx::query_as(
        "SELECT mp.id, mp.account_id, mp.status, mp.joined_at, mp.left_at,
                mp.disconnect_count,
                CASE WHEN a.email LIKE '%@airnote.guest' OR a.email LIKE '%@said.guest'
                     THEN regexp_replace(split_part(a.email, '@', 1), '-[0-9a-f]{32}$', '')
                     ELSE COALESCE(NULLIF(om.lark_name, ''), split_part(a.email, '@', 1))
                END
           FROM meeting_participants mp
           JOIN accounts a ON a.id = mp.account_id
           LEFT JOIN org_members om ON om.account_id = mp.account_id AND om.org_id = $2
          WHERE mp.meeting_id = $1
          ORDER BY mp.joined_at ASC NULLS LAST",
    )
    .bind(meeting_id)
    .bind(org_id)
    .fetch_all(&state.db)
    .await
    .map_err(db_err)?;

    let participants_json: Vec<Value> = participants
        .into_iter()
        .map(
            |(pid, account_id, pstatus, joined_at, left_at, disconnect_count, name)| {
                json!({
                    "id":               pid,
                    "account_id":       account_id,
                    "status":           pstatus,
                    "joined_at":        joined_at,
                    "left_at":          left_at,
                    "disconnect_count": disconnect_count,
                    "name":             name,
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

    // Fetch transcript with speaker names.
    // Guest accounts use app-owned emails — strip the trailing -{uuid32} suffix.
    let transcript: Vec<(String, Option<String>, String, i64, i32)> = sqlx::query_as(
        "SELECT tc.speaker_id::text,
                CASE WHEN a.email LIKE '%@airnote.guest' OR a.email LIKE '%@said.guest'
                     THEN regexp_replace(split_part(a.email, '@', 1), '-[0-9a-f]{32}$', '')
                     ELSE COALESCE(NULLIF(om.lark_name, ''), split_part(a.email, '@', 1))
                END,
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
            "scheduled_at": scheduled_at,
            "duration_minutes": duration_minutes,
            "lark_calendar_id": lark_calendar_id,
            "lark_event_id": lark_event_id,
            "lark_event_status": lark_event_status,
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
    headers: HeaderMap,
    user: AuthUser,
    Path(meeting_id): Path<Uuid>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let (_, org_id) = tenant::require_active_org(&state, &user, &headers).await?;

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
    headers: HeaderMap,
    user: AuthUser,
    Path(meeting_id): Path<Uuid>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let (_, org_id) = tenant::require_active_org(&state, &user, &headers).await?;

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

// ── DELETE /v1/meetings/:id ────────────────────────────────────────────────
// Hard-delete a meeting (children cascade via ON DELETE CASCADE). The desktop
// calls this for meetings it discarded locally as empty (immediate stop / no
// audio / killed mid-recording) so abandoned "Quick meeting" records never
// linger. Creator + org scoped; idempotent — an already-gone meeting returns ok
// so the desktop's reconcile pass can fire freely.
pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Path(meeting_id): Path<Uuid>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let (_, org_id) = tenant::require_active_org(&state, &user, &headers).await?;

    let deleted =
        sqlx::query("DELETE FROM meetings WHERE id = $1 AND org_id = $2 AND created_by = $3")
            .bind(meeting_id)
            .bind(org_id)
            .bind(user.account_id)
            .execute(&state.db)
            .await
            .map_err(db_err)?
            .rows_affected();

    Ok(Json(json!({ "deleted": deleted > 0 })))
}

// ── POST /v1/meetings/:id/push-tasks ───────────────────────────────────────

pub async fn push_tasks(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Path(meeting_id): Path<Uuid>,
    Json(body): Json<PushTasksBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let (_, org_id) = tenant::require_active_org(&state, &user, &headers).await?;

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

// ── POST /v1/meetings/:id/export-lark ─────────────────────────────────────────
// Export locally-generated meeting minutes (the desktop holds them) to a
// beautifully formatted Lark Docx, then create Lark Tasks for assigned items so
// owners get pinged and the items land on their to-do lists.

#[derive(Deserialize)]
pub struct ExportLarkItem {
    pub title: String,
    #[serde(default)]
    pub assignee: Option<String>,
}

#[derive(Deserialize)]
pub struct ExportLarkBody {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub action_items: Vec<ExportLarkItem>,
    #[serde(default)]
    pub decisions: Vec<String>,
}

/// Resolve a free-text assignee name (from the local meeting AI) to a Lark
/// user_id via org_members, best-effort: exact case-insensitive match first,
/// then a first-name / contains match. Returns None when nothing reasonable
/// matches (the item still appears in the doc, just without a Lark task).
fn resolve_lark_user_id(
    name: &str,
    members: &[(Option<String>, Option<String>)],
) -> Option<String> {
    let needle = name.trim().to_ascii_lowercase();
    if needle.is_empty() {
        return None;
    }
    // Exact (case-insensitive) name match.
    if let Some((_, Some(uid))) = members.iter().find(|(n, uid)| {
        uid.is_some()
            && n.as_deref()
                .map(|n| n.trim().eq_ignore_ascii_case(&needle))
                .unwrap_or(false)
    }) {
        return Some(uid.clone());
    }
    // First-name / substring match (one direction only, longer than 2 chars).
    members
        .iter()
        .find(|(n, uid)| {
            uid.is_some()
                && n.as_deref()
                    .map(|n| {
                        let n = n.trim().to_ascii_lowercase();
                        needle.len() > 2 && (n.starts_with(&needle) || n.contains(&needle))
                    })
                    .unwrap_or(false)
        })
        .and_then(|(_, uid)| uid.clone())
}

pub async fn export_lark(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Path(meeting_id): Path<Uuid>,
    Json(body): Json<ExportLarkBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let (_, org_id) = tenant::require_active_org(&state, &user, &headers).await?;

    // Verify the meeting belongs to this org; pull its title/time for the header.
    let meeting: Option<(String, DateTime<Utc>, i32)> = sqlx::query_as(
        "SELECT title, created_at, duration_minutes FROM meetings WHERE id = $1 AND org_id = $2",
    )
    .bind(meeting_id)
    .bind(org_id)
    .fetch_optional(&state.db)
    .await
    .map_err(db_err)?;

    let Some((db_title, created_at, duration_minutes)) = meeting else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error": "meeting not found", "code": "not_found"})),
        ));
    };

    let lark = state.lark.clone();
    if lark.app_id.is_empty() || lark.app_secret.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "Lark isn't configured for this workspace — ask an admin to connect it.",
                "code": "lark_not_configured",
            })),
        ));
    }

    let title = if body.title.trim().is_empty() {
        db_title
    } else {
        body.title.trim().to_string()
    };
    let doc_title = format!("Meeting Minutes \u{2014} {title}");
    let meta_line = format!(
        "{} \u{00B7} {} min",
        created_at.format("%b %d, %Y %H:%M UTC"),
        duration_minutes
    );

    // Org members (for assignee → Lark user_id resolution + task creation).
    let members: Vec<(Option<String>, Option<String>)> =
        sqlx::query_as("SELECT lark_name, lark_user_id FROM org_members WHERE org_id = $1")
            .bind(org_id)
            .fetch_all(&state.db)
            .await
            .map_err(db_err)?;

    let action_items: Vec<(String, Option<String>)> = body
        .action_items
        .iter()
        .map(|i| (i.title.clone(), i.assignee.clone()))
        .collect();

    let folder_token = std::env::var("LARK_MINUTES_FOLDER_TOKEN")
        .ok()
        .filter(|s| !s.trim().is_empty());

    // Create the doc as the *user* so it lands in their own Lark Drive — no
    // shared folder needed. (A shared folder is still honored if configured.)
    // Requires the caller to have signed in with Lark.
    let user_token = user_lark_token(&state, user.account_id, org_id).await?;

    // 1) Create the doc (in the user's Drive, or the shared folder if set).
    let document_id =
        crate::lark_sync::create_doc_with_token(&user_token, &doc_title, folder_token.as_deref())
            .await
            .map_err(|e| lark_error_response("could not create the Lark doc", &e))?;

    // 2) Fill it with beautifully formatted blocks.
    let blocks = crate::lark_sync::build_minutes_blocks(
        &title,
        &meta_line,
        &body.summary,
        &action_items,
        &body.decisions,
    );
    let mut content_warning: Option<String> = None;
    if let Err(e) =
        crate::lark_sync::insert_blocks_with_token(&user_token, &document_id, &blocks).await
    {
        tracing::warn!("export_lark: block insert failed for {meeting_id}: {e}");
        content_warning =
            Some("the doc was created but some formatting could not be written".into());
    }

    // 3) Ask Lark for the canonical doc URL (no hard-coded tenant domain).
    let url = crate::lark_sync::fetch_doc_url(&user_token, &document_id)
        .await
        .unwrap_or(None)
        .unwrap_or_default();

    // 4) Create Lark tasks for assigned action items so owners get pinged and
    //    the items land on their to-do lists (best-effort, never fails export).
    let mut tasks_created = 0u32;
    for item in &body.action_items {
        let Some(name) = item.assignee.as_deref() else {
            continue;
        };
        let Some(uid) = resolve_lark_user_id(name, &members) else {
            continue;
        };
        let desc = format!("From meeting: {title}\n{url}");
        match crate::lark_sync::create_lark_task(
            &lark.app_id,
            &lark.app_secret,
            &item.title,
            Some(&desc),
            Some(&uid),
        )
        .await
        {
            Ok(_) => tasks_created += 1,
            Err(e) => tracing::warn!("export_lark: task create failed for '{name}': {e}"),
        }
    }

    Ok(Json(json!({
        "ok": true,
        "url": url,
        "document_id": document_id,
        "in_shared_folder": folder_token.is_some(),
        "tasks_created": tasks_created,
        "content_warning": content_warning,
    })))
}

/// POST /v1/lark/export-doc — create a Lark Doc straight from a title + markdown
/// summary, with NO cloud meeting record. Meetings are local-only now, so the
/// desktop sends the rendered content here directly (single meeting or digest).
/// Doc-only: this is the thin Lark surface we keep server-side because it holds
/// the user's OAuth token. Reuses the same doc builder as `export_lark`.
pub async fn export_doc(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Json(body): Json<ExportLarkBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let (_, org_id) = tenant::require_active_org(&state, &user, &headers).await?;

    let lark = state.lark.clone();
    if lark.app_id.is_empty() || lark.app_secret.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "Lark isn't configured for this workspace — ask an admin to connect it.",
                "code": "lark_not_configured",
            })),
        ));
    }

    let title = if body.title.trim().is_empty() {
        "Meeting Notes".to_string()
    } else {
        body.title.trim().to_string()
    };
    let doc_title = format!("Meeting Minutes \u{2014} {title}");

    let action_items: Vec<(String, Option<String>)> = body
        .action_items
        .iter()
        .map(|i| (i.title.clone(), i.assignee.clone()))
        .collect();

    let folder_token = std::env::var("LARK_MINUTES_FOLDER_TOKEN")
        .ok()
        .filter(|s| !s.trim().is_empty());

    // Create the doc as the user so it lands in their own Lark Drive.
    let user_token = user_lark_token(&state, user.account_id, org_id).await?;

    let document_id =
        crate::lark_sync::create_doc_with_token(&user_token, &doc_title, folder_token.as_deref())
            .await
            .map_err(|e| lark_error_response("could not create the Lark doc", &e))?;

    let blocks = crate::lark_sync::build_minutes_blocks(
        &title,
        "",
        &body.summary,
        &action_items,
        &body.decisions,
    );
    let mut content_warning: Option<String> = None;
    if let Err(e) =
        crate::lark_sync::insert_blocks_with_token(&user_token, &document_id, &blocks).await
    {
        tracing::warn!("export_doc: block insert failed: {e}");
        content_warning =
            Some("the doc was created but some formatting could not be written".into());
    }

    let url = crate::lark_sync::fetch_doc_url(&user_token, &document_id)
        .await
        .unwrap_or(None)
        .unwrap_or_default();

    Ok(Json(json!({
        "ok": true,
        "url": url,
        "document_id": document_id,
        "in_shared_folder": folder_token.is_some(),
        "tasks_created": 0,
        "content_warning": content_warning,
    })))
}

/// A valid Lark *user* access_token for the caller, refreshing it when it's
/// near expiry. The doc is created under this identity so it lands in the
/// user's own Drive. Errors clearly if the account isn't linked to Lark.
async fn user_lark_token(
    state: &AppState,
    account_id: Uuid,
    org_id: Uuid,
) -> Result<String, (StatusCode, Json<Value>)> {
    let row: Option<(String, String, DateTime<Utc>)> = sqlx::query_as(
        "SELECT access_token, refresh_token, token_expires_at
           FROM lark_tokens WHERE account_id = $1 AND org_id = $2",
    )
    .bind(account_id)
    .bind(org_id)
    .fetch_optional(&state.db)
    .await
    .map_err(db_err)?;

    let Some((access_token, refresh_token, expires_at)) = row else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "Sign in with Lark to export — your account isn't linked to Lark.",
                "code": "lark_not_linked",
            })),
        ));
    };

    // Still valid (with a 2-minute safety margin)?
    if expires_at > Utc::now() + Duration::minutes(2) {
        return Ok(access_token);
    }

    // Expired/near-expiry → refresh and persist.
    let refreshed = crate::lark_client::refresh_access_token(
        &state.lark.app_id,
        &state.lark.app_secret,
        &refresh_token,
    )
    .await
    // A failed refresh almost always means the stored refresh token is expired
    // or revoked → the user must re-authorize, so surface it as reauth-required.
    .map_err(|_| {
        (
            StatusCode::FORBIDDEN,
            Json(json!({
                "error": "Your Lark session expired — reconnect Lark (Settings → Enterprise) and try again.",
                "code": "lark_reauth_required",
            })),
        )
    })?;

    let new_expires = Utc::now() + Duration::seconds(refreshed.expires_in);
    sqlx::query(
        "UPDATE lark_tokens
            SET access_token = $1, refresh_token = $2, token_expires_at = $3, updated_at = now()
          WHERE account_id = $4 AND org_id = $5",
    )
    .bind(&refreshed.access_token)
    .bind(&refreshed.refresh_token)
    .bind(new_expires)
    .bind(account_id)
    .bind(org_id)
    .execute(&state.db)
    .await
    .map_err(db_err)?;

    Ok(refreshed.access_token)
}

/// Map a Lark API error string into an HTTP response. Authorization/scope
/// failures (e.g. 99991679, "unauthorized", missing privileges) become a
/// distinct `lark_reauth_required` code so the desktop can guide the user to
/// reconnect Lark instead of showing a dead-end error.
fn lark_error_response(context: &str, e: &str) -> (StatusCode, Json<Value>) {
    let lower = e.to_lowercase();
    let needs_reauth = lower.contains("99991679")
        || lower.contains("unauthorized")
        || lower.contains("permission")
        || lower.contains("re-authorization")
        || lower.contains("privilege")
        || lower.contains("forbidden")
        || lower.contains("scope");
    if needs_reauth {
        (
            StatusCode::FORBIDDEN,
            Json(json!({
                "error": "Lark needs you to reconnect with the latest permissions — open Settings → Enterprise, sign in with Lark again, then retry.",
                "code": "lark_reauth_required",
            })),
        )
    } else {
        (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": format!("{context}: {e}"), "code": "lark_error"})),
        )
    }
}

// ── GET /v1/orgs/:org_id/meetings/costs ────────────────────────────────────────
// Unified Admin meeting feed. Historical cloud meetings and metadata-only local
// desktop sessions intentionally share this response so a migration does not
// make an organisation's earlier meeting history disappear. Role: org viewer.

#[derive(Deserialize)]
pub struct MeetingCostsQuery {
    /// Window in days, or "all"/"0" for all-time (shared telemetry semantics).
    pub days: Option<String>,
}

#[derive(sqlx::FromRow)]
struct MeetingCostRow {
    id: Uuid,
    source: String,
    title: String,
    status: String,
    started_at: DateTime<Utc>,
    ended_at: Option<DateTime<Utc>>,
    duration_seconds: f64,
    transcript_word_count: i64,
    host_account_id: Uuid,
    host_name: Option<String>,
    host_email: Option<String>,
    participant_count: i64,
    provider: Option<String>,
    model: Option<String>,
    usage_count: i64,
    input_tokens: i64,
    cached_input_tokens: i64,
    cache_miss_tokens: i64,
    output_tokens: i64,
    reasoning_tokens: i64,
    cost_usd: f64,
}

pub async fn org_meeting_costs(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Path(org_id): Path<Uuid>,
    Query(q): Query<MeetingCostsQuery>,
) -> Result<Json<Value>, StatusCode> {
    crate::routes::telemetry::require_platform_or_org_viewer(&state, &user, &headers, org_id)
        .await?;

    let (days, since) = crate::routes::telemetry::window_bounds(q.days.as_deref());

    let rows: Vec<MeetingCostRow> = sqlx::query_as(
        "SELECT * FROM (
            SELECT m.id,
                   'legacy'::text AS source,
                   m.title,
                   m.status,
                   COALESCE(m.started_at, m.created_at) AS started_at,
                   m.ended_at,
                   GREATEST(COALESCE(EXTRACT(EPOCH FROM (m.ended_at - m.started_at)), 0), 0)::float8 AS duration_seconds,
                   COALESCE(ms.transcript_word_count, 0)::bigint AS transcript_word_count,
                   m.created_by AS host_account_id,
                   COALESCE(NULLIF(om.lark_name, ''), split_part(a.email, '@', 1)) AS host_name,
                   a.email AS host_email,
                   COALESCE(pc.participant_count, 0)::bigint AS participant_count,
                   u.provider,
                   u.model,
                   COALESCE(u.usage_count, 0)::bigint AS usage_count,
                   COALESCE(u.input_tokens, 0)::bigint AS input_tokens,
                   COALESCE(u.cached_input_tokens, 0)::bigint AS cached_input_tokens,
                   GREATEST(COALESCE(u.input_tokens, 0) - COALESCE(u.cached_input_tokens, 0), 0)::bigint AS cache_miss_tokens,
                   COALESCE(u.output_tokens, 0)::bigint AS output_tokens,
                   0::bigint AS reasoning_tokens,
                   COALESCE(u.cost_usd, 0)::float8 AS cost_usd
              FROM meetings m
              LEFT JOIN accounts a ON a.id = m.created_by
              LEFT JOIN org_members om ON om.account_id = m.created_by AND om.org_id = m.org_id
              LEFT JOIN LATERAL (
                 SELECT MAX(mpu.provider) AS provider,
                        MAX(mpu.model) AS model,
                        COUNT(*)::bigint AS usage_count,
                        SUM(mpu.input_tokens)::bigint AS input_tokens,
                        SUM(mpu.cached_input_tokens)::bigint AS cached_input_tokens,
                        SUM(mpu.output_tokens)::bigint AS output_tokens,
                        SUM(mpu.estimated_cost_usd)::float8 AS cost_usd
                   FROM meeting_provider_usage mpu
                  WHERE mpu.meeting_id = m.id
              ) u ON true
              LEFT JOIN LATERAL (
                 SELECT COUNT(*)::bigint AS participant_count
                   FROM meeting_participants mp
                  WHERE mp.meeting_id = m.id
              ) pc ON true
              LEFT JOIN LATERAL (
                 SELECT SUM(slot.word_count)::bigint AS transcript_word_count
                   FROM meeting_slots slot
                  WHERE slot.meeting_id = m.id
              ) ms ON true
             WHERE m.org_id = $1
               AND ($2::timestamptz IS NULL OR COALESCE(m.started_at, m.created_at) >= $2)

            UNION ALL

            SELECT lms.id,
                   'local'::text AS source,
                   lms.title,
                   lms.status,
                   lms.started_at,
                   lms.ended_at,
                   lms.duration_seconds::float8,
                   lms.transcript_word_count::bigint,
                   lms.account_id AS host_account_id,
                   COALESCE(NULLIF(om.lark_name, ''), split_part(a.email, '@', 1)) AS host_name,
                   a.email AS host_email,
                   1::bigint AS participant_count,
                   u.provider,
                   u.model,
                   COALESCE(u.usage_count, 0)::bigint AS usage_count,
                   COALESCE(u.prompt_tokens, 0)::bigint AS input_tokens,
                   COALESCE(u.cache_hit_tokens, 0)::bigint AS cached_input_tokens,
                   COALESCE(u.cache_miss_tokens, 0)::bigint AS cache_miss_tokens,
                   COALESCE(u.completion_tokens, 0)::bigint AS output_tokens,
                   COALESCE(u.reasoning_tokens, 0)::bigint AS reasoning_tokens,
                   COALESCE(u.cost_usd, 0)::float8 AS cost_usd
              FROM local_meeting_sessions lms
              LEFT JOIN accounts a ON a.id = lms.account_id
              LEFT JOIN org_members om ON om.account_id = lms.account_id AND om.org_id = lms.org_id
              LEFT JOIN LATERAL (
                 SELECT MAX(lmpu.provider) AS provider,
                        MAX(lmpu.model) AS model,
                        COUNT(*)::bigint AS usage_count,
                        SUM(lmpu.prompt_tokens)::bigint AS prompt_tokens,
                        SUM(lmpu.cache_hit_tokens)::bigint AS cache_hit_tokens,
                        SUM(lmpu.cache_miss_tokens)::bigint AS cache_miss_tokens,
                        SUM(lmpu.completion_tokens)::bigint AS completion_tokens,
                        SUM(COALESCE(lmpu.reasoning_tokens, 0))::bigint AS reasoning_tokens,
                        SUM(lmpu.estimated_cost_usd)::float8 AS cost_usd
                   FROM local_meeting_provider_usage lmpu
                  WHERE lmpu.local_meeting_session_id = lms.id
              ) u ON true
             WHERE lms.org_id = $1
               AND ($2::timestamptz IS NULL OR lms.started_at >= $2)
        ) unified
        ORDER BY started_at DESC, cost_usd DESC",
    )
    .bind(org_id)
    .bind(since)
    .fetch_all(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let mut total_cost_usd = 0.0f64;
    let mut total_tokens: i64 = 0;
    let mut total_recording_seconds = 0.0f64;
    let mut total_transcript_words = 0i64;
    let meetings: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            total_cost_usd += r.cost_usd;
            total_tokens += r.input_tokens + r.output_tokens;
            total_recording_seconds += r.duration_seconds;
            total_transcript_words += r.transcript_word_count;
            json!({
                "id": r.id,
                "source": r.source,
                "title": r.title,
                "status": r.status,
                "started_at": r.started_at,
                "created_at": r.started_at,
                "ended_at": r.ended_at,
                "duration_seconds": r.duration_seconds,
                "transcript_word_count": r.transcript_word_count,
                "host_account_id": r.host_account_id,
                "host_name": r.host_name,
                "host_email": r.host_email,
                "participant_count": r.participant_count,
                "provider": r.provider,
                "model": r.model,
                "usage_count": r.usage_count,
                "input_tokens": r.input_tokens,
                "cached_input_tokens": r.cached_input_tokens,
                "cache_miss_tokens": r.cache_miss_tokens,
                "output_tokens": r.output_tokens,
                "reasoning_tokens": r.reasoning_tokens,
                "cost_usd": r.cost_usd,
            })
        })
        .collect();

    Ok(Json(json!({
        "window_days": days,
        "meeting_count": meetings.len(),
        "total_recording_seconds": total_recording_seconds,
        "total_transcript_words": total_transcript_words,
        "total_cost_usd": total_cost_usd,
        "total_tokens": total_tokens,
        "rate_cards": [
            {
                "model": "deepseek-v4-flash",
                "cache_hit_usd_per_million": costs::DEEPSEEK_V4_FLASH_CACHE_HIT_USD_PER_MILLION,
                "cache_miss_usd_per_million": costs::DEEPSEEK_V4_FLASH_INPUT_USD_PER_MILLION,
                "output_usd_per_million": costs::DEEPSEEK_V4_FLASH_OUTPUT_USD_PER_MILLION,
            },
            {
                "model": "deepseek-v4-pro",
                "cache_hit_usd_per_million": costs::DEEPSEEK_V4_PRO_CACHE_HIT_USD_PER_MILLION,
                "cache_miss_usd_per_million": costs::DEEPSEEK_V4_PRO_CACHE_MISS_USD_PER_MILLION,
                "output_usd_per_million": costs::DEEPSEEK_V4_PRO_OUTPUT_USD_PER_MILLION,
            }
        ],
        "meetings": meetings,
    })))
}

// ── GET /v1/orgs/:org_id/meetings/:meeting_id/cost ─────────────────────────────
// Unified metadata and per-stage AI usage detail. Role: org viewer.

pub async fn meeting_cost_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Path((org_id, meeting_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Value>, StatusCode> {
    let (_, role) = tenant::ensure_path_org_active(&state, &user, &headers, org_id)
        .await
        .map_err(|_| StatusCode::FORBIDDEN)?;
    crate::routes::telemetry::require_org_viewer(&role)?;

    let local: Option<(String, String, DateTime<Utc>, DateTime<Utc>, f64, i32, Uuid)> =
        sqlx::query_as(
            "SELECT title, status, started_at, ended_at, duration_seconds,
                    transcript_word_count, account_id
               FROM local_meeting_sessions
              WHERE id = $1 AND org_id = $2",
        )
        .bind(meeting_id)
        .bind(org_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    if let Some((title, status, started_at, ended_at, duration_seconds, words, owner_id)) = local {
        let owner: Option<(String, Option<String>)> = sqlx::query_as(
            "SELECT a.email, om.lark_name
               FROM accounts a
               LEFT JOIN org_members om ON om.account_id = a.id AND om.org_id = $2
              WHERE a.id = $1",
        )
        .bind(owner_id)
        .bind(org_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        let stages: Vec<(
            String,
            String,
            String,
            String,
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
            f64,
            f64,
        )> = sqlx::query_as(
            "SELECT feature_stage, provider, model, result_status,
                        COUNT(*)::bigint,
                        SUM(prompt_tokens)::bigint,
                        SUM(cache_hit_tokens)::bigint,
                        SUM(cache_miss_tokens)::bigint,
                        SUM(completion_tokens)::bigint,
                        SUM(COALESCE(reasoning_tokens, 0))::bigint,
                        AVG(latency_ms)::float8,
                        SUM(estimated_cost_usd)::float8
                   FROM local_meeting_provider_usage
                  WHERE local_meeting_session_id = $1 AND org_id = $2
                  GROUP BY feature_stage, provider, model, result_status
                  ORDER BY MIN(occurred_at), feature_stage",
        )
        .bind(meeting_id)
        .bind(org_id)
        .fetch_all(&state.db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        let by_stage: Vec<Value> = stages
            .into_iter()
            .map(
                |(
                    stage,
                    provider,
                    model,
                    result_status,
                    call_count,
                    input,
                    cache_hit,
                    cache_miss,
                    output,
                    reasoning,
                    latency,
                    cost,
                )| {
                    json!({
                        "stage": stage,
                        "provider": provider,
                        "model": model,
                        "result_status": result_status,
                        "call_count": call_count,
                        "input_tokens": input,
                        "cached_input_tokens": cache_hit,
                        "cache_hit_tokens": cache_hit,
                        "cache_miss_tokens": cache_miss,
                        "output_tokens": output,
                        "reasoning_tokens": reasoning,
                        "average_latency_ms": latency,
                        "cost_usd": cost,
                    })
                },
            )
            .collect();
        let (email, lark_name) = owner.unwrap_or_default();
        return Ok(Json(json!({
            "meeting_id": meeting_id,
            "source": "local",
            "title": title,
            "status": status,
            "started_at": started_at,
            "ended_at": ended_at,
            "duration_seconds": duration_seconds,
            "transcript_word_count": words,
            "host_account_id": owner_id,
            "host_name": lark_name.unwrap_or_else(|| email.split('@').next().unwrap_or(&email).to_string()),
            "host_email": email,
            "by_stage": by_stage,
        })));
    }

    let legacy: Option<(String, String, DateTime<Utc>, Option<DateTime<Utc>>, f64, i64, Uuid)> =
        sqlx::query_as(
            "SELECT m.title, m.status, COALESCE(m.started_at, m.created_at), m.ended_at,
                    GREATEST(COALESCE(EXTRACT(EPOCH FROM (m.ended_at - m.started_at)), 0), 0)::float8,
                    COALESCE((SELECT SUM(ms.word_count)::bigint FROM meeting_slots ms WHERE ms.meeting_id = m.id), 0)::bigint,
                    m.created_by
               FROM meetings m
              WHERE m.id = $1 AND m.org_id = $2",
        )
        .bind(meeting_id)
        .bind(org_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let Some((title, status, started_at, ended_at, duration_seconds, words, owner_id)) = legacy
    else {
        return Err(StatusCode::NOT_FOUND);
    };

    let totals: (i64, i64, i64, f64, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT COALESCE(SUM(input_tokens), 0)::bigint,
                COALESCE(SUM(cached_input_tokens), 0)::bigint,
                COALESCE(SUM(output_tokens), 0)::bigint,
                COALESCE(SUM(estimated_cost_usd), 0)::float8,
                MAX(provider), MAX(model)
           FROM meeting_provider_usage
          WHERE meeting_id = $1",
    )
    .bind(meeting_id)
    .fetch_one(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let slots: Vec<(Option<i32>, String, String, i64, i64, i64, i64, f64)> = sqlx::query_as(
        "SELECT slot_index,
                MAX(provider), MAX(model),
                COUNT(*)::bigint,
                COALESCE(SUM(input_tokens), 0)::bigint,
                COALESCE(SUM(cached_input_tokens), 0)::bigint,
                COALESCE(SUM(output_tokens), 0)::bigint,
                COALESCE(SUM(estimated_cost_usd), 0)::float8
           FROM meeting_provider_usage
          WHERE meeting_id = $1
          GROUP BY slot_index
          ORDER BY slot_index ASC NULLS LAST",
    )
    .bind(meeting_id)
    .fetch_all(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let by_slot: Vec<Value> = slots
        .into_iter()
        .map(|(slot_index, provider, model, call_count, input_tokens, cached_input_tokens, output_tokens, cost_usd)| {
            json!({
                "stage": slot_index.map(|index| format!("summary slot {index}")).unwrap_or_else(|| "summary".to_string()),
                "slot_index": slot_index,
                "provider": provider,
                "model": model,
                "result_status": "success",
                "call_count": call_count,
                "input_tokens": input_tokens,
                "cached_input_tokens": cached_input_tokens,
                "cache_hit_tokens": cached_input_tokens,
                "cache_miss_tokens": (input_tokens - cached_input_tokens).max(0),
                "output_tokens": output_tokens,
                "reasoning_tokens": 0,
                "cost_usd": cost_usd,
            })
        })
        .collect();

    let owner: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT a.email, om.lark_name
           FROM accounts a
           LEFT JOIN org_members om ON om.account_id = a.id AND om.org_id = $2
          WHERE a.id = $1",
    )
    .bind(owner_id)
    .bind(org_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let (email, lark_name) = owner.unwrap_or_default();

    Ok(Json(json!({
        "meeting_id": meeting_id,
        "source": "legacy",
        "title": title,
        "status": status,
        "started_at": started_at,
        "ended_at": ended_at,
        "duration_seconds": duration_seconds,
        "transcript_word_count": words,
        "host_account_id": owner_id,
        "host_name": lark_name.unwrap_or_else(|| email.split('@').next().unwrap_or(&email).to_string()),
        "host_email": email,
        "model": totals.5,
        "provider": totals.4,
        "input_tokens": totals.0,
        "cached_input_tokens": totals.1,
        "output_tokens": totals.2,
        "cost_usd": totals.3,
        "by_slot": by_slot.clone(),
        "by_stage": by_slot,
    })))
}

fn db_err(_e: sqlx::Error) -> (StatusCode, Json<Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": "database error"})),
    )
}
