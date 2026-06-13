//! Lark sync route:
//!   POST /v1/meetings/:id/sync-to-lark — sync meeting results to Lark

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{AppState, auth::AuthUser, tenant};

// ── POST /v1/meetings/:id/sync-to-lark ─────────────────────────────────────

pub async fn sync_to_lark(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Path(meeting_id): Path<Uuid>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let (_, org_id) = tenant::require_active_org(&state, &user, &headers).await?;

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

fn db_err(_e: sqlx::Error) -> (StatusCode, Json<Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": "database error"})),
    )
}
