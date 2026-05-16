//! Lark sync route:
//!   POST /v1/meetings/:id/sync-to-lark — sync meeting results to Lark

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{AppState, auth::AuthUser};

// ── POST /v1/meetings/:id/sync-to-lark ─────────────────────────────────────

pub async fn sync_to_lark(
    State(state): State<AppState>,
    user: AuthUser,
    Path(meeting_id): Path<Uuid>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    // Resolve caller's org
    let org_id = resolve_org(&state, user.account_id).await?;

    // Verify meeting exists, belongs to caller's org, and is ended
    let meeting_status: Option<String> =
        sqlx::query_scalar("SELECT status FROM meetings WHERE id = $1 AND org_id = $2")
            .bind(meeting_id)
            .bind(org_id)
            .fetch_optional(&state.db)
            .await
            .map_err(db_err)?;

    let status = meeting_status.ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "meeting not found"})),
        )
    })?;

    if status != "ended" {
        return Err((
            StatusCode::CONFLICT,
            Json(json!({"error": format!("meeting status is '{status}', must be 'ended'")})),
        ));
    }

    // Sync to Lark
    let result = crate::lark_sync::sync_meeting_to_lark(
        &state.lark.app_id,
        &state.lark.app_secret,
        meeting_id,
        &state.db,
    )
    .await
    .map_err(|e| {
        tracing::error!("sync_meeting_to_lark failed: {e}");
        (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": format!("lark sync failed: {e}")})),
        )
    })?;

    Ok(Json(json!({
        "ok": true,
        "tasks_synced": result.tasks_synced,
        "doc_id": result.doc_id,
        "messages_sent": result.messages_sent,
    })))
}

// ── Helpers ─────────────────────────────────────────────────────────────────

async fn resolve_org(
    state: &AppState,
    account_id: Uuid,
) -> Result<Uuid, (StatusCode, Json<Value>)> {
    let org_id: Option<Uuid> =
        sqlx::query_scalar("SELECT org_id FROM org_members WHERE account_id = $1 LIMIT 1")
            .bind(account_id)
            .fetch_optional(&state.db)
            .await
            .map_err(db_err)?;

    org_id.ok_or_else(|| {
        (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "you must belong to an org"})),
        )
    })
}

fn db_err(_e: sqlx::Error) -> (StatusCode, Json<Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": "database error"})),
    )
}
