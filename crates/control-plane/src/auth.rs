//! Bearer-token auth extractor.
//!
//! Routes that require a logged-in account add `AuthUser` as a parameter:
//!   ```rust
//!   async fn my_handler(user: AuthUser, ...) { ... }
//!   ```

use axum::{
    async_trait,
    extract::{FromRef, FromRequestParts},
    http::{StatusCode, request::Parts},
};
use uuid::Uuid;

use crate::AppState;

/// The authenticated account extracted from an `Authorization: Bearer <token>` header.
#[derive(Clone)]
pub struct AuthUser {
    pub account_id: Uuid,
    pub email: String,
}

#[async_trait]
impl<S> FromRequestParts<S> for AuthUser
where
    AppState: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        // ── Extract bearer token ──────────────────────────────────────────────
        let auth_header = parts
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .ok_or((StatusCode::UNAUTHORIZED, "missing authorization header"))?;

        let token_str = auth_header
            .strip_prefix("Bearer ")
            .ok_or((StatusCode::UNAUTHORIZED, "malformed authorization header"))?;

        let token = Uuid::parse_str(token_str)
            .map_err(|_| (StatusCode::UNAUTHORIZED, "invalid token format"))?;

        // ── Validate against sessions table ───────────────────────────────────
        let app = AppState::from_ref(state);
        let row: Option<(Uuid, String)> = sqlx::query_as(
            "SELECT a.id, a.email
               FROM sessions s
               JOIN accounts a ON a.id = s.account_id
              WHERE s.token = $1
                AND s.expires_at > now()",
        )
        .bind(token)
        .fetch_optional(&app.db)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "database error"))?;

        let (account_id, email) =
            row.ok_or((StatusCode::UNAUTHORIZED, "invalid or expired token"))?;

        Ok(AuthUser { account_id, email })
    }
}
