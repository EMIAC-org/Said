use axum::{Json, extract::State};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    AppState, cp_client,
    store::{company_vocab, users},
};

#[derive(Debug, Deserialize)]
pub struct SyncBody {
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Deserialize)]
pub struct UploadBody {
    pub device_id: Option<String>,
    #[serde(default)]
    pub force: bool,
}

fn server_url(raw: &str, path: &str) -> String {
    format!(
        "{}/{}",
        raw.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

pub async fn status(State(state): State<AppState>) -> Json<Value> {
    let user_id = state.default_user_id.to_string();
    let enterprise = users::get_user(&state.pool, &user_id);
    let connected = users::has_enterprise_auth(&state.pool, &user_id);
    let bucket = company_vocab::status(&state.pool, &user_id);
    Json(json!({
        "connected": connected,
        "server_url": enterprise.and_then(|u| u.enterprise_server_url),
        "bucket": bucket,
    }))
}

pub async fn sync(State(state): State<AppState>, Json(body): Json<SyncBody>) -> Json<Value> {
    let user_id = state.default_user_id.to_string();
    let Some(user) = users::get_user(&state.pool, &user_id) else {
        return Json(json!({ "ok": false, "changed": false, "error": "local user not found" }));
    };
    let token = match user.cloud_token.as_deref().filter(|t| !t.trim().is_empty()) {
        Some(t) => t.to_string(),
        None => {
            return Json(
                json!({ "ok": false, "changed": false, "error": "enterprise token missing" }),
            );
        }
    };
    let Some(base_url) = user.enterprise_server_url.as_deref().map(str::to_string) else {
        return Json(
            json!({ "ok": false, "changed": false, "error": "enterprise server URL missing" }),
        );
    };
    if !company_vocab::should_check(&state.pool, &user_id, body.force) {
        let status = company_vocab::status(&state.pool, &user_id);
        return Json(json!({ "ok": true, "changed": false, "skipped": true, "bucket": status }));
    }

    let local = company_vocab::status(&state.pool, &user_id);
    let version_url = server_url(
        &base_url,
        &format!(
            "/v1/company-vocab/version?current_version={}",
            local.version
        ),
    );
    let version_res = match cp_client::with_org_context(
        state.http_client.get(version_url).bearer_auth(&token),
        Some(&user),
    )
    .send()
    .await
    {
        Ok(res) => res,
        Err(e) => {
            let msg = format!("version check failed: {e}");
            company_vocab::mark_checked(
                &state.pool,
                &user_id,
                local.org_id.as_deref(),
                local.version,
                local.bucket_hash.as_deref(),
                Some(&msg),
            );
            return Json(json!({ "ok": false, "changed": false, "error": msg }));
        }
    };
    if !version_res.status().is_success() {
        let msg = format!("version check HTTP {}", version_res.status());
        company_vocab::mark_checked(
            &state.pool,
            &user_id,
            local.org_id.as_deref(),
            local.version,
            local.bucket_hash.as_deref(),
            Some(&msg),
        );
        return Json(json!({ "ok": false, "changed": false, "error": msg }));
    }
    let version_json: Value = match version_res.json().await {
        Ok(v) => v,
        Err(e) => {
            let msg = format!("version JSON failed: {e}");
            company_vocab::mark_checked(
                &state.pool,
                &user_id,
                local.org_id.as_deref(),
                local.version,
                local.bucket_hash.as_deref(),
                Some(&msg),
            );
            return Json(json!({ "ok": false, "changed": false, "error": msg }));
        }
    };
    let remote_version = version_json
        .get("version")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let remote_hash = version_json.get("bucket_hash").and_then(|v| v.as_str());
    let org_id = version_json.get("org_id").and_then(|v| v.as_str());
    let changed = version_json
        .get("changed")
        .and_then(|v| v.as_bool())
        .unwrap_or(remote_version > local.version);
    if !changed || remote_version <= 0 {
        company_vocab::mark_checked(
            &state.pool,
            &user_id,
            org_id.or(local.org_id.as_deref()),
            local.version.max(remote_version),
            remote_hash.or(local.bucket_hash.as_deref()),
            None,
        );
        let status = company_vocab::status(&state.pool, &user_id);
        return Json(json!({ "ok": true, "changed": false, "bucket": status }));
    }

    let bucket_url = server_url(
        &base_url,
        &format!("/v1/company-vocab/bucket?version={remote_version}"),
    );
    let bucket_res = match cp_client::with_org_context(
        state.http_client.get(bucket_url).bearer_auth(&token),
        Some(&user),
    )
    .send()
    .await
    {
        Ok(res) => res,
        Err(e) => {
            let msg = format!("bucket download failed: {e}");
            company_vocab::mark_checked(
                &state.pool,
                &user_id,
                org_id,
                local.version,
                local.bucket_hash.as_deref(),
                Some(&msg),
            );
            return Json(json!({ "ok": false, "changed": false, "error": msg }));
        }
    };
    if !bucket_res.status().is_success() {
        let msg = format!("bucket download HTTP {}", bucket_res.status());
        company_vocab::mark_checked(
            &state.pool,
            &user_id,
            org_id,
            local.version,
            local.bucket_hash.as_deref(),
            Some(&msg),
        );
        return Json(json!({ "ok": false, "changed": false, "error": msg }));
    }
    let bucket: company_vocab::CompanyBucketResponse = match bucket_res.json().await {
        Ok(v) => v,
        Err(e) => {
            let msg = format!("bucket JSON failed: {e}");
            company_vocab::mark_checked(
                &state.pool,
                &user_id,
                org_id,
                local.version,
                local.bucket_hash.as_deref(),
                Some(&msg),
            );
            return Json(json!({ "ok": false, "changed": false, "error": msg }));
        }
    };
    if let Err(e) = company_vocab::replace_bucket(&state.pool, &user_id, &bucket) {
        let msg = format!("local bucket write failed: {e}");
        company_vocab::mark_checked(
            &state.pool,
            &user_id,
            Some(&bucket.org_id),
            local.version,
            local.bucket_hash.as_deref(),
            Some(&msg),
        );
        return Json(json!({ "ok": false, "changed": false, "error": msg }));
    }
    crate::invalidate_lexicon_cache(&state.lexicon_cache).await;
    let status = company_vocab::status(&state.pool, &user_id);
    Json(json!({ "ok": true, "changed": true, "bucket": status }))
}

pub async fn upload_user_summary(
    State(state): State<AppState>,
    Json(body): Json<UploadBody>,
) -> Json<Value> {
    let user_id = state.default_user_id.to_string();
    let Some(user) = users::get_user(&state.pool, &user_id) else {
        return Json(json!({ "ok": false, "error": "local user not found" }));
    };
    let token = match user.cloud_token.as_deref().filter(|t| !t.trim().is_empty()) {
        Some(t) => t.to_string(),
        None => return Json(json!({ "ok": false, "error": "enterprise token missing" })),
    };
    let Some(base_url) = user.enterprise_server_url.as_deref().map(str::to_string) else {
        return Json(json!({ "ok": false, "error": "enterprise server URL missing" }));
    };
    if !company_vocab::should_upload_summary(&state.pool, &user_id, body.force) {
        return Json(json!({ "ok": true, "skipped": true }));
    }
    let device_id = body
        .device_id
        .filter(|d| !d.trim().is_empty())
        .unwrap_or_else(|| "unknown-device".to_string());
    let payload = company_vocab::build_user_summary(&state.pool, &user_id, device_id);
    let url = server_url(&base_url, "/v1/company-vocab/user-vocab");
    let res = match cp_client::with_org_context(
        state
            .http_client
            .post(url)
            .bearer_auth(&token)
            .json(&payload),
        Some(&user),
    )
    .send()
    .await
    {
        Ok(res) => res,
        Err(e) => {
            let msg = format!("upload failed: {e}");
            company_vocab::mark_upload_result(&state.pool, &user_id, Some(&msg));
            return Json(json!({ "ok": false, "error": msg }));
        }
    };
    if !res.status().is_success() {
        let msg = format!("upload HTTP {}", res.status());
        company_vocab::mark_upload_result(&state.pool, &user_id, Some(&msg));
        return Json(json!({ "ok": false, "error": msg }));
    }
    company_vocab::mark_upload_result(&state.pool, &user_id, None);
    Json(json!({
        "ok": true,
        "terms": payload.terms.len(),
        "aliases": payload.aliases.len(),
    }))
}
