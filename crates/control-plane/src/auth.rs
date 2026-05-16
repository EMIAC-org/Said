//! Bearer-token auth extractor.
//!
//! Accepts BOTH token formats:
//!   1. Session UUID — looked up in the `sessions` table
//!   2. JWT — decoded and verified against `jwt_secret`, then account looked up
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
use jsonwebtoken::{DecodingKey, Validation, decode};
use serde::Deserialize;
use uuid::Uuid;

use crate::AppState;

/// The authenticated account extracted from an `Authorization: Bearer <token>` header.
#[derive(Clone)]
pub struct AuthUser {
    pub account_id: Uuid,
    pub email: String,
}

#[derive(Deserialize)]
struct JwtClaims {
    sub: String,
    email: String,
}

/// Try to resolve a bearer token to an account. Checks session UUID first,
/// then falls back to JWT decode.
async fn resolve_token(token_str: &str, app: &AppState) -> Option<(Uuid, String)> {
    // 1. Try as session UUID
    if let Ok(token_uuid) = Uuid::parse_str(token_str) {
        let row: Option<(Uuid, String)> = sqlx::query_as(
            "SELECT a.id, a.email
               FROM sessions s
               JOIN accounts a ON a.id = s.account_id
              WHERE s.token = $1
                AND s.expires_at > now()",
        )
        .bind(token_uuid)
        .fetch_optional(&app.db)
        .await
        .ok()?;

        if let Some(r) = row {
            return Some(r);
        }
    }

    // 2. Try as JWT
    let key = DecodingKey::from_secret(app.lark.jwt_secret.as_bytes());
    let mut validation = Validation::default();
    validation.validate_exp = true;

    let token_data = decode::<JwtClaims>(token_str, &key, &validation).ok()?;
    let account_id = Uuid::parse_str(&token_data.claims.sub).ok()?;

    // Verify account still exists
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM accounts WHERE id = $1)")
        .bind(account_id)
        .fetch_one(&app.db)
        .await
        .ok()?;

    if exists {
        Some((account_id, token_data.claims.email))
    } else {
        None
    }
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
        let (account_id, email) = resolve_token(token_str, &app)
            .await
            .ok_or((StatusCode::UNAUTHORIZED, "invalid or expired token"))?;

        Ok(AuthUser { account_id, email })
    }
}

/// Standalone function for WebSocket auth (where we have the token as a string,
/// not in an HTTP header).
pub async fn resolve_ws_token(token_str: &str, state: &AppState) -> Option<(Uuid, String)> {
    resolve_token(token_str, state).await
}
