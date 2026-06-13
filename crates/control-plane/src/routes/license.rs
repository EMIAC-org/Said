//! GET /v1/license/check
//!
//! Returns the caller's current license tier, features, and limits.
//! Workspace users receive org subscription limits; personal users use account license.

use axum::{Json, extract::State, http::HeaderMap};
use serde_json::{Value, json};

use crate::{AppState, auth::AuthUser, org_quota, routes::auth::license_features, tenant};

pub async fn check(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: AuthUser,
) -> Json<Value> {
    let tenant_ctx = tenant::resolve_tenant(&state, &user, &headers).await.ok();

    if let Some(ref ctx) = tenant_ctx {
        if let Some(org_id) = ctx.active_org_id {
            let tier = org_quota::org_tier(&state, org_id)
                .await
                .unwrap_or_else(|_| "team".into());
            let features = license_features(&tier);
            let daily_limit = org_quota::org_daily_polish_limit(&tier);
            let used = org_quota::org_polish_count_today(&state, org_id)
                .await
                .unwrap_or(0);

            return Json(json!({
                "tier": tier,
                "active": true,
                "features": features,
                "scope": "org",
                "org_id": org_id,
                "limits": {
                    "daily_polishes": daily_limit,
                    "used_today": used,
                },
            }));
        }
    }

    let tier: String = sqlx::query_scalar(
        "SELECT tier FROM license_keys
          WHERE account_id = $1 AND active = true
            AND (expires_at IS NULL OR expires_at > now())
          ORDER BY created_at DESC LIMIT 1",
    )
    .bind(user.account_id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten()
    .unwrap_or_else(|| "free".into());

    let features = license_features(&tier);
    let daily_limit = match tier.as_str() {
        "pro" => 500,
        "team" => 2000,
        _ => 50,
    };

    Json(json!({
        "tier": tier,
        "active": true,
        "features": features,
        "scope": "personal",
        "limits": {
            "daily_polishes": daily_limit,
        },
    }))
}
