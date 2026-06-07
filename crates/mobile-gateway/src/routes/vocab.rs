//! Personal vocabulary snapshot, explicit add-term, and learn-spelling feedback.
//!
//!   GET  /v1/mobile/vocab/snapshot   — ETag-cacheable personal vocab
//!   POST /v1/mobile/vocab/terms      — add/update a term explicitly
//!   POST /v1/mobile/feedback         — explicit "learn spelling" (v1 learning)
//!
//! Learning is explicit-only in v1 and never blocks insertion. Raw transcripts
//! are not stored — only the canonical term the user chose to keep.

use axum::{
    Json,
    extract::{Query, State},
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{AppState, auth::AuthUser, runtime, util::*};

#[derive(Debug, Deserialize)]
pub struct SnapshotQuery {
    #[serde(default)]
    pub hash: Option<String>,
}

pub async fn snapshot(
    State(state): State<AppState>,
    user: AuthUser,
    Query(q): Query<SnapshotQuery>,
) -> ApiResult<Json<Value>> {
    let (hash, terms) = runtime::vocab::load_snapshot(&state.db, user.account_id).await;
    if q.hash.as_deref() == Some(hash.as_str()) {
        return Ok(Json(json!({ "hash": hash, "unchanged": true })));
    }
    Ok(Json(json!({
        "schema": "airnote.mobile.vocab.v1",
        "hash": hash,
        "unchanged": false,
        "terms": terms,
        "style_defaults": { "default_style": "work", "language_hint": "hinglish" }
    })))
}

#[derive(Debug, Deserialize)]
pub struct AddTermBody {
    pub term: String,
    #[serde(default)]
    pub spoken_aliases: Vec<String>,
    #[serde(default)]
    pub term_type: Option<String>,
}

pub async fn add_term(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<AddTermBody>,
) -> ApiResult<Json<Value>> {
    let term = clean_required(&body.term, 120, "term")?;
    let aliases: Vec<String> = body
        .spoken_aliases
        .iter()
        .map(|a| a.trim().to_string())
        .filter(|a| !a.is_empty())
        .take(12)
        .collect();
    let account_id = user.account_id;
    upsert_term(&state, account_id, &term, &aliases, body.term_type.as_deref()).await?;
    let hash = runtime::vocab::current_hash(&state.db, account_id).await;
    Ok(Json(json!({ "ok": true, "term": term, "hash": hash })))
}

#[derive(Debug, Deserialize)]
pub struct FeedbackBody {
    #[serde(default)]
    pub action: Option<String>,
    #[serde(default)]
    pub user_kept: Option<String>,
}

/// Explicit learning entry point. Only `learn_spelling` mutates vocab in v1.
pub async fn feedback(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<FeedbackBody>,
) -> ApiResult<Json<Value>> {
    let action = body.action.unwrap_or_default();
    if action == "learn_spelling" {
        if let Some(kept) = body.user_kept.as_deref() {
            let term = clean_required(kept, 120, "user_kept")?;
            upsert_term(&state, user.account_id, &term, &[], None).await?;
            return Ok(Json(json!({ "ok": true, "learned": term })));
        }
    }
    // Other feedback actions are accepted but not acted on in v1.
    Ok(Json(json!({ "ok": true, "learned": Value::Null })))
}

// ── Helpers ───────────────────────────────────────────────────────────────────

async fn upsert_term(
    state: &AppState,
    account_id: uuid::Uuid,
    term: &str,
    aliases: &[String],
    term_type: Option<&str>,
) -> ApiResult<()> {
    sqlx::query(
        "INSERT INTO vocab_terms (account_id, term, spoken_aliases, term_type, source)
         VALUES ($1, $2, $3, $4, 'user')
         ON CONFLICT (account_id, term) DO UPDATE
           SET spoken_aliases = EXCLUDED.spoken_aliases,
               term_type = COALESCE(EXCLUDED.term_type, vocab_terms.term_type),
               archived_at = NULL,
               updated_at = now()",
    )
    .bind(account_id)
    .bind(term)
    .bind(json!(aliases))
    .bind(term_type)
    .execute(&state.db)
    .await
    .map_err(db_err)?;
    Ok(())
}
