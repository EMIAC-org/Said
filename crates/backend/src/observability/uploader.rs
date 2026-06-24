//! Best-effort background uploader for observability outbox rows.

use crate::observability::outbox::{
    AliasBatchPayload, DictationPatchPayload, DictationUpsertPayload, OutboxRow, list_pending,
    mark_done, mark_failed, pending_count,
};
use crate::store::DbPool;
use reqwest::Client;
use serde::Serialize;
use tracing::{debug, warn};

const BATCH_LIMIT: i64 = 20;

fn base_url(user: &crate::store::users::LocalUser) -> String {
    user.enterprise_server_url
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| "https://airnote.emiactech.com".to_string())
}

async fn post_json(
    http: &Client,
    token: &str,
    url: &str,
    body: &impl Serialize,
) -> Result<(), String> {
    let resp = http
        .post(url)
        .bearer_auth(token)
        .json(body)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if resp.status().is_success() {
        Ok(())
    } else {
        Err(format!("HTTP {}", resp.status()))
    }
}

async fn patch_json(
    http: &Client,
    token: &str,
    url: &str,
    body: &impl Serialize,
) -> Result<(), String> {
    let resp = http
        .patch(url)
        .bearer_auth(token)
        .json(body)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if resp.status().is_success() {
        Ok(())
    } else {
        Err(format!("HTTP {}", resp.status()))
    }
}

async fn upload_row(http: &Client, token: &str, base: &str, row: &OutboxRow) -> Result<(), String> {
    let base = base.trim_end_matches('/');
    match row.op.as_str() {
        "upsert_dictation" => {
            let payload: DictationUpsertPayload =
                serde_json::from_str(&row.payload_json).map_err(|e| e.to_string())?;
            post_json(
                http,
                token,
                &format!("{base}/v1/runtime/observability/dictation"),
                &payload,
            )
            .await
        }
        "patch_dictation_edit" => {
            let payload: DictationPatchPayload =
                serde_json::from_str(&row.payload_json).map_err(|e| e.to_string())?;
            let recording_id = payload.recording_id.clone();
            patch_json(
                http,
                token,
                &format!("{base}/v1/runtime/observability/dictation/{recording_id}"),
                &payload,
            )
            .await
        }
        "upsert_alias_batch" => {
            let payload: AliasBatchPayload =
                serde_json::from_str(&row.payload_json).map_err(|e| e.to_string())?;
            post_json(
                http,
                token,
                &format!("{base}/v1/runtime/observability/aliases"),
                &payload,
            )
            .await
        }
        other => Err(format!("unknown outbox op: {other}")),
    }
}

pub async fn upload_pending(pool: &DbPool, user_id: &str, http: &Client) {
    let user = match crate::store::users::get_user(pool, user_id) {
        Some(u) => u,
        None => return,
    };
    let Some(token) = user
        .cloud_token
        .as_deref()
        .filter(|t| !t.trim().is_empty())
        .map(str::to_string)
    else {
        debug!("[observability] no cloud token — skipping upload");
        return;
    };

    let rows = match list_pending(pool, user_id, BATCH_LIMIT) {
        Ok(r) => r,
        Err(e) => {
            warn!("[observability] list_pending failed: {e}");
            return;
        }
    };
    if rows.is_empty() {
        return;
    }

    let base = base_url(&user);
    for row in rows {
        match upload_row(http, &token, &base, &row).await {
            Ok(()) => {
                if let Err(e) = mark_done(pool, row.id) {
                    warn!("[observability] mark_done failed: {e}");
                }
            }
            Err(e) => {
                if let Err(me) = mark_failed(pool, row.id, &e) {
                    warn!("[observability] mark_failed failed: {me}");
                }
            }
        }
    }
}

pub fn spawn_uploader(pool: DbPool, user_id: String, http: Client) {
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(20)).await;
        upload_pending(&pool, &user_id, &http).await;

        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            interval.tick().await;
            if pending_count(&pool, &user_id) > 0 {
                upload_pending(&pool, &user_id, &http).await;
            }
        }
    });
}

pub fn maybe_upload_after_enqueue(pool: &DbPool, user_id: &str, http: &Client) {
    if pending_count(pool, user_id) == 0 {
        return;
    }
    let pool = pool.clone();
    let user_id = user_id.to_string();
    let http = http.clone();
    tokio::spawn(async move {
        upload_pending(&pool, &user_id, &http).await;
    });
}
