//! Company vocabulary bucket APIs.
//!
//! These routes deliberately store only vocabulary summaries from desktop
//! clients. Raw transcripts, polished output, and surrounding context are
//! rejected at the API boundary.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{AppState, auth::AuthUser};

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub status: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TermBody {
    pub term: String,
    pub term_type: Option<String>,
    pub language: Option<String>,
    pub weight: Option<f64>,
    pub priority: Option<i32>,
    pub status: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AliasBody {
    pub transcript_form: String,
    pub correct_form: String,
    pub language: Option<String>,
    pub weight: Option<f64>,
    pub status: Option<String>,
    pub safety_status: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PublishBody {
    pub notes: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SuggestionActionBody {
    pub action: String,
}

#[derive(Debug, Deserialize)]
pub struct DesktopVersionQuery {
    pub current_version: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct DesktopBucketQuery {
    pub version: Option<i32>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct UserVocabTermUpload {
    pub term: String,
    pub term_norm: Option<String>,
    pub term_type: Option<String>,
    pub source: Option<String>,
    pub weight: Option<f64>,
    pub use_count: Option<i32>,
    pub positive_count: Option<i32>,
    pub negative_count: Option<i32>,
    pub safety_status: Option<String>,
    pub first_seen_at: Option<Value>,
    pub last_seen_at: Option<Value>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct UserVocabAliasUpload {
    pub transcript_form: String,
    pub transcript_norm: Option<String>,
    pub correct_form: String,
    pub correct_norm: Option<String>,
    pub weight: Option<f64>,
    pub use_count: Option<i32>,
    pub positive_count: Option<i32>,
    pub negative_count: Option<i32>,
    pub safety_status: Option<String>,
    pub review_status: Option<String>,
    pub first_seen_at: Option<Value>,
    pub last_seen_at: Option<Value>,
}

fn parse_upload_time(value: &Option<Value>) -> Option<DateTime<Utc>> {
    value.as_ref().and_then(|v| {
        if let Some(s) = v.as_str() {
            DateTime::parse_from_rfc3339(s)
                .ok()
                .map(|dt| dt.with_timezone(&Utc))
        } else {
            None
        }
    })
}

#[derive(Debug, Deserialize)]
pub struct UserVocabUpload {
    pub device_id: String,
    #[serde(default)]
    pub terms: Vec<UserVocabTermUpload>,
    #[serde(default)]
    pub aliases: Vec<UserVocabAliasUpload>,
    pub company_bucket_version: Option<i32>,
    pub company_vocab_synced_at: Option<Value>,
}

fn normalize(raw: &str) -> String {
    raw.trim()
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c
            } else if c.is_whitespace() || matches!(c, '-' | '_' | '.') {
                ' '
            } else {
                '\0'
            }
        })
        .filter(|c| *c != '\0')
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn common_phrase(norm: &str) -> bool {
    const COMMON: &[&str] = &[
        "kaisa",
        "kaisi",
        "kaise",
        "aisa",
        "aisi",
        "aise",
        "laga",
        "lagi",
        "lage",
        "main",
        "mein",
        "hai",
        "hain",
        "tha",
        "thi",
        "the",
        "time",
        "can",
        "go",
        "do",
        "this",
        "for",
        "me",
        "one thing",
        "ek baar",
        "char log",
        "kaam",
        "kya",
        "kyun",
        "aur",
        "batao",
        "bolo",
    ];
    if COMMON.contains(&norm) {
        return true;
    }
    let tokens: Vec<_> = norm.split_whitespace().collect();
    !tokens.is_empty() && tokens.iter().all(|t| COMMON.contains(t))
}

fn safety_status_for_source(norm: &str) -> &'static str {
    if common_phrase(norm) {
        "common_block"
    } else if norm.is_empty() {
        "ambiguous_block"
    } else {
        "safe_jargon"
    }
}

fn valid_status(raw: Option<String>, default_status: &str) -> String {
    match raw.as_deref() {
        Some("approved" | "draft" | "blocked" | "rejected" | "pending") => raw.unwrap(),
        _ => default_status.to_string(),
    }
}

fn db_err(e: sqlx::Error) -> (StatusCode, Json<Value>) {
    tracing::warn!("[company-vocab] db error: {e}");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": "database error"})),
    )
}

fn bad_request(message: &'static str) -> (StatusCode, Json<Value>) {
    (StatusCode::BAD_REQUEST, Json(json!({"error": message})))
}

async fn resolve_org_role(
    state: &AppState,
    account_id: Uuid,
) -> Result<(Uuid, String), (StatusCode, Json<Value>)> {
    let row: Option<(Uuid, String)> =
        sqlx::query_as("SELECT org_id, role FROM org_members WHERE account_id = $1 LIMIT 1")
            .bind(account_id)
            .fetch_optional(&state.db)
            .await
            .map_err(db_err)?;
    row.ok_or_else(|| {
        (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "you must belong to an org"})),
        )
    })
}

fn require_viewer(role: &str) -> Result<(), (StatusCode, Json<Value>)> {
    if role.eq_ignore_ascii_case("admin")
        || role.eq_ignore_ascii_case("company_admin")
        || role.eq_ignore_ascii_case("manager")
        || role.eq_ignore_ascii_case("member")
    {
        Ok(())
    } else {
        Err((
            StatusCode::FORBIDDEN,
            Json(json!({"error": "insufficient permissions"})),
        ))
    }
}

fn require_admin(role: &str) -> Result<(), (StatusCode, Json<Value>)> {
    if role.eq_ignore_ascii_case("admin")
        || role.eq_ignore_ascii_case("company_admin")
        || role.eq_ignore_ascii_case("manager")
    {
        Ok(())
    } else {
        Err((
            StatusCode::FORBIDDEN,
            Json(json!({"error": "admin permissions required"})),
        ))
    }
}

async fn ensure_org_access(
    state: &AppState,
    user: &AuthUser,
    org_id: Uuid,
    admin: bool,
) -> Result<String, (StatusCode, Json<Value>)> {
    let (member_org, role) = resolve_org_role(state, user.account_id).await?;
    if member_org != org_id {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({"error": "not a member of this org"})),
        ));
    }
    if admin {
        require_admin(&role)?;
    } else {
        require_viewer(&role)?;
    }
    Ok(role)
}

async fn audit(
    state: &AppState,
    org_id: Uuid,
    actor_id: Uuid,
    action: &str,
    entity_type: &str,
    entity_id: Option<Uuid>,
    payload: Value,
) {
    let _ = sqlx::query(
        "INSERT INTO org_vocab_audit_log (org_id, actor_id, action, entity_type, entity_id, payload)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(org_id)
    .bind(actor_id)
    .bind(action)
    .bind(entity_type)
    .bind(entity_id)
    .bind(payload)
    .execute(&state.db)
    .await;
}

pub async fn list_terms(
    State(state): State<AppState>,
    user: AuthUser,
    Path(org_id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    ensure_org_access(&state, &user, org_id, false).await?;
    let status = query.status.unwrap_or_else(|| "all".to_string());
    let rows = if status == "all" {
        sqlx::query_as::<_, (Uuid, String, String, String, String, f64, i32, String, DateTime<Utc>)>(
            "SELECT id, term, term_norm, term_type, language, weight, priority, status, updated_at
               FROM org_vocab_terms WHERE org_id = $1 ORDER BY priority DESC, updated_at DESC, term ASC",
        )
        .bind(org_id)
        .fetch_all(&state.db)
        .await
        .map_err(db_err)?
    } else {
        sqlx::query_as::<
            _,
            (
                Uuid,
                String,
                String,
                String,
                String,
                f64,
                i32,
                String,
                DateTime<Utc>,
            ),
        >(
            "SELECT id, term, term_norm, term_type, language, weight, priority, status, updated_at
               FROM org_vocab_terms WHERE org_id = $1 AND status = $2
              ORDER BY priority DESC, updated_at DESC, term ASC",
        )
        .bind(org_id)
        .bind(status)
        .fetch_all(&state.db)
        .await
        .map_err(db_err)?
    };
    Ok(Json(json!({ "terms": rows.into_iter().map(|r| json!({
        "id": r.0,
        "term": r.1,
        "term_norm": r.2,
        "term_type": r.3,
        "language": r.4,
        "weight": r.5,
        "priority": r.6,
        "status": r.7,
        "updated_at": r.8,
    })).collect::<Vec<_>>() })))
}

pub async fn create_term(
    State(state): State<AppState>,
    user: AuthUser,
    Path(org_id): Path<Uuid>,
    Json(body): Json<TermBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    ensure_org_access(&state, &user, org_id, true).await?;
    let term = body.term.trim();
    if term.is_empty() {
        return Err(bad_request("term required"));
    }
    let term_norm = normalize(term);
    let status = valid_status(body.status, "draft");
    let term_type = body.term_type.unwrap_or_else(|| "other".to_string());
    let language = body.language.unwrap_or_else(|| "hinglish".to_string());
    let row: (Uuid,) = sqlx::query_as(
        "INSERT INTO org_vocab_terms
            (org_id, term, term_norm, term_type, language, weight, priority, status, created_by, updated_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $9)
         ON CONFLICT (org_id, term_norm) DO UPDATE
           SET term = EXCLUDED.term,
               term_type = EXCLUDED.term_type,
               language = EXCLUDED.language,
               weight = EXCLUDED.weight,
               priority = EXCLUDED.priority,
               status = EXCLUDED.status,
               updated_by = EXCLUDED.updated_by,
               updated_at = now()
         RETURNING id",
    )
    .bind(org_id)
    .bind(term)
    .bind(term_norm)
    .bind(term_type)
    .bind(language)
    .bind(body.weight.unwrap_or(1.0))
    .bind(body.priority.unwrap_or(0))
    .bind(status)
    .bind(user.account_id)
    .fetch_one(&state.db)
    .await
    .map_err(db_err)?;
    audit(
        &state,
        org_id,
        user.account_id,
        "upsert",
        "term",
        Some(row.0),
        json!({ "term": term }),
    )
    .await;
    Ok(Json(json!({ "id": row.0 })))
}

pub async fn update_term(
    State(state): State<AppState>,
    user: AuthUser,
    Path((org_id, term_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<TermBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    ensure_org_access(&state, &user, org_id, true).await?;
    let term = body.term.trim();
    if term.is_empty() {
        return Err(bad_request("term required"));
    }
    let status = valid_status(body.status, "draft");
    sqlx::query(
        "UPDATE org_vocab_terms
            SET term = $3,
                term_norm = $4,
                term_type = $5,
                language = $6,
                weight = $7,
                priority = $8,
                status = $9,
                updated_by = $10,
                updated_at = now()
          WHERE org_id = $1 AND id = $2",
    )
    .bind(org_id)
    .bind(term_id)
    .bind(term)
    .bind(normalize(term))
    .bind(body.term_type.unwrap_or_else(|| "other".to_string()))
    .bind(body.language.unwrap_or_else(|| "hinglish".to_string()))
    .bind(body.weight.unwrap_or(1.0))
    .bind(body.priority.unwrap_or(0))
    .bind(status)
    .bind(user.account_id)
    .execute(&state.db)
    .await
    .map_err(db_err)?;
    audit(
        &state,
        org_id,
        user.account_id,
        "update",
        "term",
        Some(term_id),
        json!({ "term": term }),
    )
    .await;
    Ok(Json(json!({ "ok": true })))
}

pub async fn delete_term(
    State(state): State<AppState>,
    user: AuthUser,
    Path((org_id, term_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    ensure_org_access(&state, &user, org_id, true).await?;
    sqlx::query("UPDATE org_vocab_terms SET status = 'blocked', updated_by = $3, updated_at = now() WHERE org_id = $1 AND id = $2")
        .bind(org_id)
        .bind(term_id)
        .bind(user.account_id)
        .execute(&state.db)
        .await
        .map_err(db_err)?;
    audit(
        &state,
        org_id,
        user.account_id,
        "block",
        "term",
        Some(term_id),
        json!({}),
    )
    .await;
    Ok(Json(json!({ "ok": true })))
}

pub async fn list_aliases(
    State(state): State<AppState>,
    user: AuthUser,
    Path(org_id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    ensure_org_access(&state, &user, org_id, false).await?;
    let status = query.status.unwrap_or_else(|| "all".to_string());
    let rows = if status == "all" {
        sqlx::query_as::<
            _,
            (
                Uuid,
                String,
                String,
                String,
                String,
                String,
                f64,
                String,
                String,
                DateTime<Utc>,
            ),
        >(
            "SELECT id, transcript_form, transcript_norm, correct_form, correct_norm, language,
                    weight, status, safety_status, updated_at
               FROM org_vocab_aliases WHERE org_id = $1 ORDER BY updated_at DESC",
        )
        .bind(org_id)
        .fetch_all(&state.db)
        .await
        .map_err(db_err)?
    } else {
        sqlx::query_as::<
            _,
            (
                Uuid,
                String,
                String,
                String,
                String,
                String,
                f64,
                String,
                String,
                DateTime<Utc>,
            ),
        >(
            "SELECT id, transcript_form, transcript_norm, correct_form, correct_norm, language,
                    weight, status, safety_status, updated_at
               FROM org_vocab_aliases WHERE org_id = $1 AND status = $2 ORDER BY updated_at DESC",
        )
        .bind(org_id)
        .bind(status)
        .fetch_all(&state.db)
        .await
        .map_err(db_err)?
    };
    Ok(Json(json!({ "aliases": rows.into_iter().map(|r| json!({
        "id": r.0,
        "transcript_form": r.1,
        "transcript_norm": r.2,
        "correct_form": r.3,
        "correct_norm": r.4,
        "language": r.5,
        "weight": r.6,
        "status": r.7,
        "safety_status": r.8,
        "updated_at": r.9,
    })).collect::<Vec<_>>() })))
}

pub async fn create_alias(
    State(state): State<AppState>,
    user: AuthUser,
    Path(org_id): Path<Uuid>,
    Json(body): Json<AliasBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    ensure_org_access(&state, &user, org_id, true).await?;
    let transcript = body.transcript_form.trim();
    let correct = body.correct_form.trim();
    if transcript.is_empty() || correct.is_empty() {
        return Err(bad_request("transcript_form and correct_form required"));
    }
    let transcript_norm = normalize(transcript);
    let correct_norm = normalize(correct);
    let safety = body
        .safety_status
        .unwrap_or_else(|| safety_status_for_source(&transcript_norm).to_string());
    if safety == "common_block" && body.status.as_deref() != Some("blocked") {
        return Err(bad_request("common source aliases must be blocked"));
    }
    let status = valid_status(body.status, "draft");
    let row: (Uuid,) = sqlx::query_as(
        "INSERT INTO org_vocab_aliases
            (org_id, transcript_form, transcript_norm, correct_form, correct_norm, language,
             weight, status, safety_status, created_by, updated_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $10)
         ON CONFLICT (org_id, transcript_norm, correct_norm) DO UPDATE
           SET transcript_form = EXCLUDED.transcript_form,
               correct_form = EXCLUDED.correct_form,
               language = EXCLUDED.language,
               weight = EXCLUDED.weight,
               status = EXCLUDED.status,
               safety_status = EXCLUDED.safety_status,
               updated_by = EXCLUDED.updated_by,
               updated_at = now()
         RETURNING id",
    )
    .bind(org_id)
    .bind(transcript)
    .bind(transcript_norm)
    .bind(correct)
    .bind(correct_norm)
    .bind(body.language.unwrap_or_else(|| "hinglish".to_string()))
    .bind(body.weight.unwrap_or(1.0))
    .bind(status)
    .bind(safety)
    .bind(user.account_id)
    .fetch_one(&state.db)
    .await
    .map_err(db_err)?;
    audit(
        &state,
        org_id,
        user.account_id,
        "upsert",
        "alias",
        Some(row.0),
        json!({ "transcript_form": transcript, "correct_form": correct }),
    )
    .await;
    Ok(Json(json!({ "id": row.0 })))
}

pub async fn update_alias(
    State(state): State<AppState>,
    user: AuthUser,
    Path((org_id, alias_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<AliasBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    ensure_org_access(&state, &user, org_id, true).await?;
    let transcript = body.transcript_form.trim();
    let correct = body.correct_form.trim();
    if transcript.is_empty() || correct.is_empty() {
        return Err(bad_request("transcript_form and correct_form required"));
    }
    let transcript_norm = normalize(transcript);
    let safety = body
        .safety_status
        .unwrap_or_else(|| safety_status_for_source(&transcript_norm).to_string());
    if safety == "common_block" && body.status.as_deref() != Some("blocked") {
        return Err(bad_request("common source aliases must be blocked"));
    }
    sqlx::query(
        "UPDATE org_vocab_aliases
            SET transcript_form = $3,
                transcript_norm = $4,
                correct_form = $5,
                correct_norm = $6,
                language = $7,
                weight = $8,
                status = $9,
                safety_status = $10,
                updated_by = $11,
                updated_at = now()
          WHERE org_id = $1 AND id = $2",
    )
    .bind(org_id)
    .bind(alias_id)
    .bind(transcript)
    .bind(transcript_norm)
    .bind(correct)
    .bind(normalize(correct))
    .bind(body.language.unwrap_or_else(|| "hinglish".to_string()))
    .bind(body.weight.unwrap_or(1.0))
    .bind(valid_status(body.status, "draft"))
    .bind(safety)
    .bind(user.account_id)
    .execute(&state.db)
    .await
    .map_err(db_err)?;
    audit(
        &state,
        org_id,
        user.account_id,
        "update",
        "alias",
        Some(alias_id),
        json!({}),
    )
    .await;
    Ok(Json(json!({ "ok": true })))
}

pub async fn delete_alias(
    State(state): State<AppState>,
    user: AuthUser,
    Path((org_id, alias_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    ensure_org_access(&state, &user, org_id, true).await?;
    sqlx::query("UPDATE org_vocab_aliases SET status = 'blocked', updated_by = $3, updated_at = now() WHERE org_id = $1 AND id = $2")
        .bind(org_id)
        .bind(alias_id)
        .bind(user.account_id)
        .execute(&state.db)
        .await
        .map_err(db_err)?;
    audit(
        &state,
        org_id,
        user.account_id,
        "block",
        "alias",
        Some(alias_id),
        json!({}),
    )
    .await;
    Ok(Json(json!({ "ok": true })))
}

async fn build_manifest(
    state: &AppState,
    org_id: Uuid,
) -> Result<Value, (StatusCode, Json<Value>)> {
    let terms = sqlx::query_as::<_, (String, String, String, String, f64, i32)>(
        "SELECT term, term_norm, term_type, language, weight, priority
           FROM org_vocab_terms
          WHERE org_id = $1 AND status = 'approved'
          ORDER BY priority DESC, term ASC",
    )
    .bind(org_id)
    .fetch_all(&state.db)
    .await
    .map_err(db_err)?;

    let aliases = sqlx::query_as::<_, (String, String, String, String, String, f64, String)>(
        "SELECT transcript_form, transcript_norm, correct_form, correct_norm, language, weight, safety_status
           FROM org_vocab_aliases
          WHERE org_id = $1 AND status = 'approved' AND safety_status <> 'common_block'
          ORDER BY updated_at DESC",
    )
    .bind(org_id)
    .fetch_all(&state.db)
    .await
    .map_err(db_err)?;

    Ok(json!({
        "schema_version": 1,
        "terms": terms.into_iter().map(|r| json!({
            "term": r.0,
            "term_norm": r.1,
            "term_type": r.2,
            "language": r.3,
            "weight": r.4,
            "priority": r.5,
            "source": "company",
        })).collect::<Vec<_>>(),
        "aliases": aliases.into_iter().map(|r| json!({
            "transcript_form": r.0,
            "transcript_norm": r.1,
            "correct_form": r.2,
            "correct_norm": r.3,
            "language": r.4,
            "weight": r.5,
            "safety_status": r.6,
            "source": "company",
        })).collect::<Vec<_>>(),
    }))
}

fn manifest_hash(manifest: &Value) -> String {
    let encoded = serde_json::to_vec(manifest).unwrap_or_default();
    let digest = Sha256::digest(encoded);
    URL_SAFE_NO_PAD.encode(digest)
}

pub async fn publish(
    State(state): State<AppState>,
    user: AuthUser,
    Path(org_id): Path<Uuid>,
    Json(body): Json<PublishBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    ensure_org_access(&state, &user, org_id, true).await?;
    let manifest = build_manifest(&state, org_id).await?;
    let hash = manifest_hash(&manifest);
    let version: i32 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(version), 0) + 1 FROM org_vocab_releases WHERE org_id = $1",
    )
    .bind(org_id)
    .fetch_one(&state.db)
    .await
    .map_err(db_err)?;
    let row: (Uuid,) = sqlx::query_as(
        "INSERT INTO org_vocab_releases (org_id, version, bucket_hash, manifest_json, notes, published_by)
         VALUES ($1, $2, $3, $4, $5, $6)
         RETURNING id",
    )
    .bind(org_id)
    .bind(version)
    .bind(&hash)
    .bind(&manifest)
    .bind(body.notes)
    .bind(user.account_id)
    .fetch_one(&state.db)
    .await
    .map_err(db_err)?;
    audit(
        &state,
        org_id,
        user.account_id,
        "publish",
        "release",
        Some(row.0),
        json!({ "version": version, "hash": hash }),
    )
    .await;
    Ok(Json(
        json!({ "id": row.0, "version": version, "hash": hash, "manifest": manifest }),
    ))
}

pub async fn releases(
    State(state): State<AppState>,
    user: AuthUser,
    Path(org_id): Path<Uuid>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    ensure_org_access(&state, &user, org_id, false).await?;
    let rows = sqlx::query_as::<_, (Uuid, i32, String, Option<String>, DateTime<Utc>)>(
        "SELECT id, version, bucket_hash, notes, created_at
           FROM org_vocab_releases WHERE org_id = $1 ORDER BY version DESC LIMIT 25",
    )
    .bind(org_id)
    .fetch_all(&state.db)
    .await
    .map_err(db_err)?;
    Ok(Json(json!({ "releases": rows.into_iter().map(|r| json!({
        "id": r.0,
        "version": r.1,
        "bucket_hash": r.2,
        "notes": r.3,
        "created_at": r.4,
    })).collect::<Vec<_>>() })))
}

pub async fn desktop_version(
    State(state): State<AppState>,
    user: AuthUser,
    Query(query): Query<DesktopVersionQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let (org_id, _) = resolve_org_role(&state, user.account_id).await?;
    let latest: Option<(i32, String, DateTime<Utc>)> = sqlx::query_as(
        "SELECT version, bucket_hash, created_at
           FROM org_vocab_releases WHERE org_id = $1 ORDER BY version DESC LIMIT 1",
    )
    .bind(org_id)
    .fetch_optional(&state.db)
    .await
    .map_err(db_err)?;
    let Some((version, hash, published_at)) = latest else {
        return Ok(Json(
            json!({ "org_id": org_id, "version": 0, "changed": false }),
        ));
    };
    Ok(Json(json!({
        "org_id": org_id,
        "version": version,
        "bucket_hash": hash,
        "published_at": published_at,
        "changed": query.current_version.unwrap_or(0) < version,
    })))
}

pub async fn desktop_bucket(
    State(state): State<AppState>,
    user: AuthUser,
    Query(query): Query<DesktopBucketQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let (org_id, _) = resolve_org_role(&state, user.account_id).await?;
    let row: Option<(i32, String, Value, DateTime<Utc>)> = if let Some(version) = query.version {
        sqlx::query_as(
            "SELECT version, bucket_hash, manifest_json, created_at
               FROM org_vocab_releases WHERE org_id = $1 AND version = $2",
        )
        .bind(org_id)
        .bind(version)
        .fetch_optional(&state.db)
        .await
        .map_err(db_err)?
    } else {
        sqlx::query_as(
            "SELECT version, bucket_hash, manifest_json, created_at
               FROM org_vocab_releases WHERE org_id = $1 ORDER BY version DESC LIMIT 1",
        )
        .bind(org_id)
        .fetch_optional(&state.db)
        .await
        .map_err(db_err)?
    };
    let Some((version, hash, manifest, published_at)) = row else {
        return Ok(Json(
            json!({ "org_id": org_id, "version": 0, "bucket_hash": null, "manifest": {"schema_version": 1, "terms": [], "aliases": []} }),
        ));
    };
    Ok(Json(json!({
        "org_id": org_id,
        "version": version,
        "bucket_hash": hash,
        "published_at": published_at,
        "manifest": manifest,
    })))
}

fn contains_forbidden_upload_key(value: &Value) -> bool {
    const FORBIDDEN: &[&str] = &[
        "transcript",
        "raw_transcript",
        "local_corrected_transcript",
        "polished_output",
        "ai_output",
        "user_kept",
        "context",
        "example_context",
        "screen_context",
        "sentence",
    ];
    match value {
        Value::Object(map) => map
            .iter()
            .any(|(k, v)| FORBIDDEN.contains(&k.as_str()) || contains_forbidden_upload_key(v)),
        Value::Array(items) => items.iter().any(contains_forbidden_upload_key),
        _ => false,
    }
}

pub async fn upload_user_vocab(
    State(state): State<AppState>,
    user: AuthUser,
    Json(raw): Json<Value>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if contains_forbidden_upload_key(&raw) {
        return Err(bad_request(
            "raw transcript/context fields are not accepted",
        ));
    }
    let body: UserVocabUpload =
        serde_json::from_value(raw).map_err(|_| bad_request("invalid upload body"))?;
    let (org_id, _) = resolve_org_role(&state, user.account_id).await?;
    let device_id = body.device_id.trim();
    if device_id.is_empty() {
        return Err(bad_request("device_id required"));
    }

    for term in &body.terms {
        let term_text = term.term.trim();
        if term_text.is_empty() {
            continue;
        }
        let term_norm = term
            .term_norm
            .clone()
            .unwrap_or_else(|| normalize(term_text));
        sqlx::query(
            "INSERT INTO org_user_vocab_items
                (org_id, account_id, device_id, term, term_norm, term_type, source, weight,
                 use_count, positive_count, negative_count, safety_status, first_seen_at, last_seen_at)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)
             ON CONFLICT (org_id, account_id, device_id, term_norm) DO UPDATE
               SET term = EXCLUDED.term,
                   term_type = EXCLUDED.term_type,
                   source = EXCLUDED.source,
                   weight = EXCLUDED.weight,
                   use_count = EXCLUDED.use_count,
                   positive_count = EXCLUDED.positive_count,
                   negative_count = EXCLUDED.negative_count,
                   safety_status = EXCLUDED.safety_status,
                   first_seen_at = COALESCE(org_user_vocab_items.first_seen_at, EXCLUDED.first_seen_at),
                   last_seen_at = EXCLUDED.last_seen_at,
                   updated_at = now()",
        )
        .bind(org_id)
        .bind(user.account_id)
        .bind(device_id)
        .bind(term_text)
        .bind(term_norm)
        .bind(term.term_type.as_deref().unwrap_or("other"))
        .bind(term.source.as_deref().unwrap_or("local"))
        .bind(term.weight.unwrap_or(1.0))
        .bind(term.use_count.unwrap_or(0))
        .bind(term.positive_count.unwrap_or(0))
        .bind(term.negative_count.unwrap_or(0))
        .bind(term.safety_status.as_deref().unwrap_or("unknown"))
        .bind(parse_upload_time(&term.first_seen_at))
        .bind(parse_upload_time(&term.last_seen_at))
        .execute(&state.db)
        .await
        .map_err(db_err)?;
    }

    for alias in &body.aliases {
        let source = alias.transcript_form.trim();
        let correct = alias.correct_form.trim();
        if source.is_empty() || correct.is_empty() {
            continue;
        }
        let transcript_norm = alias
            .transcript_norm
            .clone()
            .unwrap_or_else(|| normalize(source));
        let correct_norm = alias
            .correct_norm
            .clone()
            .unwrap_or_else(|| normalize(correct));
        sqlx::query(
            "INSERT INTO org_user_vocab_aliases
                (org_id, account_id, device_id, transcript_form, transcript_norm, correct_form,
                 correct_norm, weight, use_count, positive_count, negative_count, safety_status,
                 review_status, first_seen_at, last_seen_at)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)
             ON CONFLICT (org_id, account_id, device_id, transcript_norm, correct_norm) DO UPDATE
               SET transcript_form = EXCLUDED.transcript_form,
                   correct_form = EXCLUDED.correct_form,
                   weight = EXCLUDED.weight,
                   use_count = EXCLUDED.use_count,
                   positive_count = EXCLUDED.positive_count,
                   negative_count = EXCLUDED.negative_count,
                   safety_status = EXCLUDED.safety_status,
                   review_status = EXCLUDED.review_status,
                   first_seen_at = COALESCE(org_user_vocab_aliases.first_seen_at, EXCLUDED.first_seen_at),
                   last_seen_at = EXCLUDED.last_seen_at,
                   updated_at = now()",
        )
        .bind(org_id)
        .bind(user.account_id)
        .bind(device_id)
        .bind(source)
        .bind(transcript_norm)
        .bind(correct)
        .bind(correct_norm)
        .bind(alias.weight.unwrap_or(1.0))
        .bind(alias.use_count.unwrap_or(0))
        .bind(alias.positive_count.unwrap_or(0))
        .bind(alias.negative_count.unwrap_or(0))
        .bind(alias.safety_status.as_deref().unwrap_or("unknown"))
        .bind(alias.review_status.as_deref().unwrap_or("unknown"))
        .bind(parse_upload_time(&alias.first_seen_at))
        .bind(parse_upload_time(&alias.last_seen_at))
        .execute(&state.db)
        .await
        .map_err(db_err)?;
    }

    sqlx::query(
        "UPDATE desktop_clients
            SET company_bucket_version = COALESCE($4, company_bucket_version),
                company_vocab_synced_at = COALESCE($5, company_vocab_synced_at),
                personal_vocab_count = $6,
                personal_alias_count = $7,
                last_seen_at = now()
          WHERE org_id = $1 AND account_id = $2 AND device_id = $3",
    )
    .bind(org_id)
    .bind(user.account_id)
    .bind(device_id)
    .bind(body.company_bucket_version)
    .bind(parse_upload_time(&body.company_vocab_synced_at))
    .bind(body.terms.len() as i32)
    .bind(body.aliases.len() as i32)
    .execute(&state.db)
    .await
    .map_err(db_err)?;

    Ok(Json(
        json!({ "ok": true, "terms": body.terms.len(), "aliases": body.aliases.len() }),
    ))
}

pub async fn user_vocab_detail(
    State(state): State<AppState>,
    user: AuthUser,
    Path((org_id, account_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    ensure_org_access(&state, &user, org_id, false).await?;
    let terms = sqlx::query_as::<
        _,
        (
            String,
            String,
            String,
            String,
            f64,
            i32,
            i32,
            i32,
            String,
            Option<DateTime<Utc>>,
        ),
    >(
        "SELECT term, term_norm, term_type, source, weight, use_count, positive_count,
                negative_count, safety_status, last_seen_at
           FROM org_user_vocab_items
          WHERE org_id = $1 AND account_id = $2
          ORDER BY weight DESC, use_count DESC, term ASC LIMIT 200",
    )
    .bind(org_id)
    .bind(account_id)
    .fetch_all(&state.db)
    .await
    .map_err(db_err)?;
    let aliases = sqlx::query_as::<
        _,
        (
            String,
            String,
            String,
            String,
            f64,
            i32,
            i32,
            i32,
            String,
            String,
            Option<DateTime<Utc>>,
        ),
    >(
        "SELECT transcript_form, transcript_norm, correct_form, correct_norm, weight, use_count,
                positive_count, negative_count, safety_status, review_status, last_seen_at
           FROM org_user_vocab_aliases
          WHERE org_id = $1 AND account_id = $2
          ORDER BY weight DESC, use_count DESC, correct_form ASC LIMIT 200",
    )
    .bind(org_id)
    .bind(account_id)
    .fetch_all(&state.db)
    .await
    .map_err(db_err)?;
    Ok(Json(json!({
        "terms": terms.into_iter().map(|r| json!({
            "term": r.0, "term_norm": r.1, "term_type": r.2, "source": r.3,
            "weight": r.4, "use_count": r.5, "positive_count": r.6,
            "negative_count": r.7, "safety_status": r.8, "last_seen_at": r.9,
        })).collect::<Vec<_>>(),
        "aliases": aliases.into_iter().map(|r| json!({
            "transcript_form": r.0, "transcript_norm": r.1,
            "correct_form": r.2, "correct_norm": r.3,
            "weight": r.4, "use_count": r.5, "positive_count": r.6,
            "negative_count": r.7, "safety_status": r.8, "review_status": r.9,
            "last_seen_at": r.10,
        })).collect::<Vec<_>>(),
    })))
}

pub async fn aggregate_suggestions_for_org(
    state: &AppState,
    org_id: Uuid,
) -> Result<(u64, u64), (StatusCode, Json<Value>)> {
    let term_rows = sqlx::query_as::<_, (String, String, String, i64, i64, i64, f64)>(
        "SELECT term_norm,
                MAX(term) AS term,
                MAX(term_type) AS term_type,
                COUNT(DISTINCT account_id) AS users_count,
                COALESCE(SUM(positive_count + use_count), 0) AS positive_total,
                COALESCE(SUM(negative_count), 0) AS negative_total,
                AVG(weight) AS weight_avg
           FROM org_user_vocab_items
          WHERE org_id = $1
          GROUP BY term_norm",
    )
    .bind(org_id)
    .fetch_all(&state.db)
    .await
    .map_err(db_err)?;

    let mut term_count = 0;
    for (term_norm, term, term_type, users_count, positive_total, negative_total, weight_avg) in
        term_rows
    {
        if term_norm.is_empty() || common_phrase(&term_norm) {
            continue;
        }
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM org_vocab_terms WHERE org_id = $1 AND term_norm = $2 AND status <> 'blocked')",
        )
        .bind(org_id)
        .bind(&term_norm)
        .fetch_one(&state.db)
        .await
        .map_err(db_err)?;
        if exists {
            continue;
        }
        let key = format!("term:{term_norm}");
        let confidence =
            (users_count as f64 * 0.35 + positive_total as f64 * 0.08 + weight_avg * 0.2)
                - negative_total as f64 * 0.2;
        let result = sqlx::query(
            "INSERT INTO org_vocab_suggestions
                (org_id, suggestion_key, kind, term, term_norm, term_type, users_count,
                 total_positive_count, total_negative_count, confidence, safety_status)
             VALUES ($1,$2,'term',$3,$4,$5,$6,$7,$8,$9,'safe_jargon')
             ON CONFLICT (org_id, suggestion_key) DO UPDATE
               SET users_count = EXCLUDED.users_count,
                   total_positive_count = EXCLUDED.total_positive_count,
                   total_negative_count = EXCLUDED.total_negative_count,
                   confidence = EXCLUDED.confidence,
                   updated_at = now()
             WHERE org_vocab_suggestions.status = 'pending'",
        )
        .bind(org_id)
        .bind(key)
        .bind(term)
        .bind(term_norm)
        .bind(term_type)
        .bind(users_count as i32)
        .bind(positive_total as i32)
        .bind(negative_total as i32)
        .bind(confidence)
        .execute(&state.db)
        .await
        .map_err(db_err)?;
        term_count += result.rows_affected();
    }

    let alias_rows = sqlx::query_as::<_, (String, String, String, String, i64, i64, i64, f64)>(
        "SELECT transcript_norm,
                MAX(transcript_form) AS transcript_form,
                correct_norm,
                MAX(correct_form) AS correct_form,
                COUNT(DISTINCT account_id) AS users_count,
                COALESCE(SUM(positive_count + use_count), 0) AS positive_total,
                COALESCE(SUM(negative_count), 0) AS negative_total,
                AVG(weight) AS weight_avg
           FROM org_user_vocab_aliases
          WHERE org_id = $1
          GROUP BY transcript_norm, correct_norm",
    )
    .bind(org_id)
    .fetch_all(&state.db)
    .await
    .map_err(db_err)?;

    let mut alias_count = 0;
    for (
        source_norm,
        source,
        correct_norm,
        correct,
        users_count,
        positive_total,
        negative_total,
        weight_avg,
    ) in alias_rows
    {
        let safety = safety_status_for_source(&source_norm);
        if safety != "safe_jargon" {
            continue;
        }
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM org_vocab_aliases WHERE org_id = $1 AND transcript_norm = $2 AND correct_norm = $3 AND status <> 'blocked')",
        )
        .bind(org_id)
        .bind(&source_norm)
        .bind(&correct_norm)
        .fetch_one(&state.db)
        .await
        .map_err(db_err)?;
        if exists {
            continue;
        }
        let key = format!("alias:{source_norm}->{correct_norm}");
        let confidence =
            (users_count as f64 * 0.45 + positive_total as f64 * 0.1 + weight_avg * 0.2)
                - negative_total as f64 * 0.25;
        let result = sqlx::query(
            "INSERT INTO org_vocab_suggestions
                (org_id, suggestion_key, kind, transcript_form, transcript_norm, correct_form,
                 correct_norm, users_count, total_positive_count, total_negative_count,
                 confidence, safety_status)
             VALUES ($1,$2,'alias',$3,$4,$5,$6,$7,$8,$9,$10,$11)
             ON CONFLICT (org_id, suggestion_key) DO UPDATE
               SET users_count = EXCLUDED.users_count,
                   total_positive_count = EXCLUDED.total_positive_count,
                   total_negative_count = EXCLUDED.total_negative_count,
                   confidence = EXCLUDED.confidence,
                   safety_status = EXCLUDED.safety_status,
                   updated_at = now()
             WHERE org_vocab_suggestions.status = 'pending'",
        )
        .bind(org_id)
        .bind(key)
        .bind(source)
        .bind(source_norm)
        .bind(correct)
        .bind(correct_norm)
        .bind(users_count as i32)
        .bind(positive_total as i32)
        .bind(negative_total as i32)
        .bind(confidence)
        .bind(safety)
        .execute(&state.db)
        .await
        .map_err(db_err)?;
        alias_count += result.rows_affected();
    }

    Ok((term_count, alias_count))
}

pub async fn aggregate_all_orgs(state: &AppState) -> Result<(u64, u64), (StatusCode, Json<Value>)> {
    let orgs: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM orgs")
        .fetch_all(&state.db)
        .await
        .map_err(db_err)?;
    let mut terms = 0;
    let mut aliases = 0;
    for org_id in orgs {
        let (t, a) = aggregate_suggestions_for_org(state, org_id).await?;
        terms += t;
        aliases += a;
    }
    Ok((terms, aliases))
}

pub async fn aggregate_now(
    State(state): State<AppState>,
    user: AuthUser,
    Path(org_id): Path<Uuid>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    ensure_org_access(&state, &user, org_id, true).await?;
    let (terms, aliases) = aggregate_suggestions_for_org(&state, org_id).await?;
    Ok(Json(
        json!({ "term_suggestions": terms, "alias_suggestions": aliases }),
    ))
}

pub async fn list_suggestions(
    State(state): State<AppState>,
    user: AuthUser,
    Path(org_id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    ensure_org_access(&state, &user, org_id, false).await?;
    let status = query.status.unwrap_or_else(|| "pending".to_string());
    let rows = sqlx::query_as::<_, (Uuid, String, Option<String>, Option<String>, Option<String>, Option<String>, i32, i32, i32, f64, String, String, DateTime<Utc>)>(
        "SELECT id, kind, term, transcript_form, correct_form, term_type, users_count,
                total_positive_count, total_negative_count, confidence, safety_status, status, updated_at
           FROM org_vocab_suggestions
          WHERE org_id = $1 AND ($2 = 'all' OR status = $2)
          ORDER BY status ASC, confidence DESC, updated_at DESC LIMIT 300",
    )
    .bind(org_id)
    .bind(status)
    .fetch_all(&state.db)
    .await
    .map_err(db_err)?;
    Ok(Json(
        json!({ "suggestions": rows.into_iter().map(|r| json!({
        "id": r.0, "kind": r.1, "term": r.2, "transcript_form": r.3,
        "correct_form": r.4, "term_type": r.5, "users_count": r.6,
        "total_positive_count": r.7, "total_negative_count": r.8,
        "confidence": r.9, "safety_status": r.10, "status": r.11,
        "updated_at": r.12,
    })).collect::<Vec<_>>() }),
    ))
}

pub async fn update_suggestion(
    State(state): State<AppState>,
    user: AuthUser,
    Path((org_id, suggestion_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<SuggestionActionBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    ensure_org_access(&state, &user, org_id, true).await?;
    let action = body.action.trim();
    if !matches!(action, "approve" | "reject" | "block") {
        return Err(bad_request("action must be approve, reject, or block"));
    }
    let row: Option<(
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        String,
    )> = sqlx::query_as(
        "SELECT kind, term, term_norm, term_type, transcript_form, correct_form, safety_status
               FROM org_vocab_suggestions WHERE org_id = $1 AND id = $2",
    )
    .bind(org_id)
    .bind(suggestion_id)
    .fetch_optional(&state.db)
    .await
    .map_err(db_err)?;
    let Some((kind, term, term_norm, term_type, transcript_form, correct_form, safety_status)) =
        row
    else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error": "suggestion not found"})),
        ));
    };

    let next_status = match action {
        "approve" => "approved",
        "reject" => "rejected",
        "block" => "blocked",
        _ => unreachable!(),
    };
    sqlx::query(
        "UPDATE org_vocab_suggestions
            SET status = $3, reviewed_by = $4, reviewed_at = now(), updated_at = now()
          WHERE org_id = $1 AND id = $2",
    )
    .bind(org_id)
    .bind(suggestion_id)
    .bind(next_status)
    .bind(user.account_id)
    .execute(&state.db)
    .await
    .map_err(db_err)?;

    if action == "approve" {
        if kind == "term" {
            if let (Some(term), Some(term_norm)) = (term, term_norm) {
                sqlx::query(
                    "INSERT INTO org_vocab_terms
                        (org_id, term, term_norm, term_type, status, created_by, updated_by)
                     VALUES ($1,$2,$3,$4,'draft',$5,$5)
                     ON CONFLICT (org_id, term_norm) DO UPDATE
                       SET status = 'draft', updated_by = EXCLUDED.updated_by, updated_at = now()",
                )
                .bind(org_id)
                .bind(term)
                .bind(term_norm)
                .bind(term_type.unwrap_or_else(|| "other".to_string()))
                .bind(user.account_id)
                .execute(&state.db)
                .await
                .map_err(db_err)?;
            }
        } else if kind == "alias" {
            if let (Some(source), Some(correct)) = (transcript_form, correct_form) {
                let source_norm = normalize(&source);
                let correct_norm = normalize(&correct);
                if safety_status == "common_block" {
                    return Err(bad_request("common source aliases cannot be approved"));
                }
                sqlx::query(
                    "INSERT INTO org_vocab_aliases
                        (org_id, transcript_form, transcript_norm, correct_form, correct_norm,
                         safety_status, status, created_by, updated_by)
                     VALUES ($1,$2,$3,$4,$5,$6,'draft',$7,$7)
                     ON CONFLICT (org_id, transcript_norm, correct_norm) DO UPDATE
                       SET status = 'draft',
                           safety_status = EXCLUDED.safety_status,
                           updated_by = EXCLUDED.updated_by,
                           updated_at = now()",
                )
                .bind(org_id)
                .bind(source)
                .bind(source_norm)
                .bind(correct)
                .bind(correct_norm)
                .bind(safety_status)
                .bind(user.account_id)
                .execute(&state.db)
                .await
                .map_err(db_err)?;
            }
        }
    }
    audit(
        &state,
        org_id,
        user.account_id,
        action,
        "suggestion",
        Some(suggestion_id),
        json!({}),
    )
    .await;
    Ok(Json(json!({ "ok": true, "status": next_status })))
}
