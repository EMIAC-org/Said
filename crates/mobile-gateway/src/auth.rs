//! Bearer-token auth extractor for the mobile gateway.
//!
//! Tokens are opaque access-session UUIDs stored in `auth_sessions` (kind
//! `access`). This service owns its own accounts and never calls the
//! control-plane to authenticate — full isolation from the desktop/enterprise
//! backend.
//!
//! Routes that require a logged-in account add `AuthUser` as a parameter:
//!   ```text
//!   async fn handler(user: AuthUser, ...) { ... }
//!   ```

use axum::{
    async_trait,
    extract::{FromRef, FromRequestParts},
    http::{StatusCode, request::Parts},
};
use uuid::Uuid;

use crate::AppState;

/// The authenticated account extracted from `Authorization: Bearer <token>`.
#[derive(Clone)]
pub struct AuthUser {
    pub account_id: Uuid,
    pub email: String,
}

/// Resolve an access-token string to `(account_id, email)` if it is a live,
/// non-expired access session.
pub async fn resolve_access_token(token_str: &str, app: &AppState) -> Option<(Uuid, String)> {
    let token_uuid = Uuid::parse_str(token_str.trim()).ok()?;
    let row: Option<(Uuid, String)> = sqlx::query_as(
        "SELECT a.id, a.email
           FROM auth_sessions s
           JOIN accounts a ON a.id = s.account_id
          WHERE s.token = $1
            AND s.kind = 'access'
            AND s.expires_at > now()",
    )
    .bind(token_uuid)
    .fetch_optional(&app.db)
    .await
    .ok()?;
    row
}

#[async_trait]
impl<S> FromRequestParts<S> for AuthUser
where
    AppState: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let auth_header = parts
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .ok_or((StatusCode::UNAUTHORIZED, "missing authorization header"))?;

        let token_str = auth_header
            .strip_prefix("Bearer ")
            .ok_or((StatusCode::UNAUTHORIZED, "malformed authorization header"))?;

        let app = AppState::from_ref(state);
        let (account_id, email) = resolve_access_token(token_str, &app)
            .await
            .ok_or((StatusCode::UNAUTHORIZED, "invalid or expired token"))?;

        Ok(AuthUser { account_id, email })
    }
}
