//! POST /v1/metering/report
//!
//! Receives aggregate usage counts from the local backend daemon.
//! No user content is sent to THIS endpoint — only counts and dates. (Transcript
//! text is persisted separately, via /v1/runtime/history/sync → the
//! `runtime_history_items` table; don't read this line as a global "server stores
//! no content" claim.)
//!
//! Body: { "events": [{ "date": "YYYY-MM-DD", "polish_count": n, "word_count": n, "model": "fast" }] }

use axum::{Json, extract::State, http::HeaderMap, http::StatusCode};
use chrono::NaiveDate;
use serde::Deserialize;
use tracing::debug;

use crate::{AppState, auth::AuthUser, org_quota, tenant};

#[derive(Deserialize)]
pub struct MeteringReport {
    pub events: Vec<UsageEvent>,
}

#[derive(Deserialize)]
pub struct UsageEvent {
    pub date: String, // "YYYY-MM-DD"
    pub polish_count: i32,
    pub word_count: i32,
    pub model: String,
}

pub async fn report(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
    Json(body): Json<MeteringReport>,
) -> StatusCode {
    let tenant = match tenant::resolve_tenant(&state, &user, &headers).await {
        Ok(t) => t,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR,
    };

    for event in &body.events {
        let Ok(date) = NaiveDate::parse_from_str(&event.date, "%Y-%m-%d") else {
            continue;
        };

        let result = sqlx::query(
            "INSERT INTO usage_events (account_id, event_date, polish_count, word_count, model_used)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (account_id, event_date, model_used) DO UPDATE
               SET polish_count = usage_events.polish_count + EXCLUDED.polish_count,
                   word_count   = usage_events.word_count   + EXCLUDED.word_count",
        )
        .bind(user.account_id)
        .bind(date)
        .bind(event.polish_count)
        .bind(event.word_count)
        .bind(&event.model)
        .execute(&state.db)
        .await;

        if let Err(e) = result {
            debug!("[metering] account upsert failed: {e}");
        }

        if let Some(org_id) = tenant.active_org_id {
            if let Err(e) = org_quota::record_org_usage(
                &state,
                org_id,
                user.account_id,
                date,
                event.polish_count,
                event.word_count,
                &event.model,
            )
            .await
            {
                debug!("[metering] org upsert failed: {e}");
            }
        }
    }

    StatusCode::NO_CONTENT
}
