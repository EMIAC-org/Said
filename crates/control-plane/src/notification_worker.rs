//! Background worker that sends Lark IM notifications for upcoming and
//! in-progress meetings.
//!
//! Runs on a 30-second interval and checks two conditions:
//! 1. **15-minute reminders** — meetings starting in ~15 minutes get a blue
//!    "Starting Soon" card.
//! 2. **Urgent join nudges** — live meetings where invited participants haven't
//!    connected after 2 minutes get a red "You're Missing It" card.

use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::LarkConfig;
use crate::lark_sync::{MeetingCardKind, send_meeting_card};
use crate::meeting_hub::MeetingHub;

pub fn start_notification_worker(db: PgPool, lark: LarkConfig, hub: Arc<MeetingHub>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(30));

        loop {
            interval.tick().await;

            if lark.app_id.is_empty() {
                continue;
            }

            if let Err(e) = check_reminders(&db, &lark).await {
                error!("[notify-worker] check_reminders failed: {e}");
            }

            if let Err(e) = check_urgent_joins(&db, &lark, &hub).await {
                error!("[notify-worker] check_urgent_joins failed: {e}");
            }
        }
    });
}

// ── 15-minute reminder ──────────────────────────────────────────────────────

async fn check_reminders(db: &PgPool, lark: &LarkConfig) -> Result<(), String> {
    let meetings: Vec<(
        Uuid,
        String,
        Uuid,
        Option<chrono::DateTime<chrono::Utc>>,
        i32,
        String,
    )> = sqlx::query_as(
        "SELECT m.id, m.title, m.org_id, m.scheduled_at, m.duration_minutes, COALESCE(m.agenda, '')
           FROM meetings m
          WHERE m.status = 'scheduled'
            AND m.scheduled_at IS NOT NULL
            AND m.scheduled_at BETWEEN now() + interval '4 minutes'
                                    AND now() + interval '6 minutes'
            AND NOT EXISTS (
                SELECT 1 FROM meeting_notifications mn
                 WHERE mn.meeting_id = m.id
                   AND mn.notification_type = 'reminder_5m'
            )",
    )
    .fetch_all(db)
    .await
    .map_err(|e| format!("query upcoming meetings: {e}"))?;

    if meetings.is_empty() {
        return Ok(());
    }

    info!(
        "[notify-worker] found {} meeting(s) needing 5-min reminder",
        meetings.len()
    );

    for (meeting_id, title, org_id, scheduled_at, duration_minutes, agenda) in &meetings {
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
        .map_err(|e| format!("query participants: {e}"))?;

        let participant_names: Vec<String> = participants
            .iter()
            .filter_map(|(_, _, name)| name.clone())
            .collect();

        for (account_id, lark_user_id, _name) in &participants {
            let Some(luid) = lark_user_id else { continue };

            if let Err(e) = send_meeting_card(
                &lark.app_id,
                &lark.app_secret,
                luid,
                title,
                agenda,
                &participant_names,
                &[],
                *scheduled_at,
                *duration_minutes,
                MeetingCardKind::StartingSoon,
            )
            .await
            {
                warn!("[notify-worker] reminder to {luid} failed: {e}");
                continue;
            }

            let _ = sqlx::query(
                "INSERT INTO meeting_notifications (meeting_id, account_id, notification_type)
                 VALUES ($1, $2, 'reminder_5m')
                 ON CONFLICT (meeting_id, account_id, notification_type) DO NOTHING",
            )
            .bind(meeting_id)
            .bind(account_id)
            .execute(db)
            .await;
        }
    }

    Ok(())
}

// ── Urgent join nudge ───────────────────────────────────────────────────────

async fn check_urgent_joins(
    db: &PgPool,
    lark: &LarkConfig,
    hub: &Arc<MeetingHub>,
) -> Result<(), String> {
    let meetings: Vec<(
        Uuid,
        String,
        Uuid,
        Option<chrono::DateTime<chrono::Utc>>,
        i32,
        String,
    )> = sqlx::query_as(
        "SELECT m.id, m.title, m.org_id, m.scheduled_at, m.duration_minutes, COALESCE(m.agenda, '')
           FROM meetings m
          WHERE m.status = 'live'
            AND m.started_at IS NOT NULL
            AND m.started_at < now() - interval '2 minutes'
            AND m.started_at > now() - interval '24 hours'
            AND NOT EXISTS (
                SELECT 1 FROM meeting_notifications mn
                 WHERE mn.meeting_id = m.id
                   AND mn.notification_type = 'urgent_join'
            )",
    )
    .fetch_all(db)
    .await
    .map_err(|e| format!("query live meetings: {e}"))?;

    if meetings.is_empty() {
        return Ok(());
    }

    info!(
        "[notify-worker] found {} live meeting(s) to check for absent participants",
        meetings.len()
    );

    for (meeting_id, title, org_id, scheduled_at, duration_minutes, agenda) in &meetings {
        let connected: std::collections::HashSet<Uuid> = hub
            .connected_participants(*meeting_id)
            .await
            .into_iter()
            .collect();

        let invited: Vec<(Uuid, Option<String>, Option<String>)> = sqlx::query_as(
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
        .map_err(|e| format!("query invited participants: {e}"))?;

        let participant_names: Vec<String> = invited
            .iter()
            .filter_map(|(_, _, name)| name.clone())
            .collect();

        for (account_id, lark_user_id, _name) in &invited {
            if connected.contains(account_id) {
                continue;
            }

            let Some(luid) = lark_user_id else { continue };

            if let Err(e) = send_meeting_card(
                &lark.app_id,
                &lark.app_secret,
                luid,
                title,
                agenda,
                &participant_names,
                &[],
                *scheduled_at,
                *duration_minutes,
                MeetingCardKind::UrgentJoin,
            )
            .await
            {
                warn!("[notify-worker] urgent to {luid} failed: {e}");
                continue;
            }

            let _ = sqlx::query(
                "INSERT INTO meeting_notifications (meeting_id, account_id, notification_type)
                 VALUES ($1, $2, 'urgent_join')
                 ON CONFLICT (meeting_id, account_id, notification_type) DO NOTHING",
            )
            .bind(meeting_id)
            .bind(account_id)
            .execute(db)
            .await;
        }
    }

    Ok(())
}
