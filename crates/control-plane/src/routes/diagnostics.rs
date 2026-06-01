//! Anonymous fleet diagnostics ingestion + admin listing.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::FromRow;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{AppState, auth::AuthUser};

const MAX_EVENTS_PER_REQUEST: usize = 50;
const MAX_DEVICE_ID_LEN: usize = 160;
const MAX_EVENT_TYPE_LEN: usize = 120;
const MAX_PHASE_LEN: usize = 80;
const RATE_LIMIT_PER_MINUTE: u32 = 120;

#[derive(Clone)]
pub struct DiagnosticsRateLimiter {
    inner: Arc<Mutex<HashMap<String, RateBucket>>>,
}

impl Default for DiagnosticsRateLimiter {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

struct RateBucket {
    window_start: Instant,
    count: u32,
}

impl DiagnosticsRateLimiter {
    pub async fn allow(&self, key: &str) -> bool {
        let mut map = self.inner.lock().await;
        let now = Instant::now();
        let bucket = map.entry(key.to_string()).or_insert(RateBucket {
            window_start: now,
            count: 0,
        });
        if now.duration_since(bucket.window_start) > Duration::from_secs(60) {
            bucket.window_start = now;
            bucket.count = 0;
        }
        if bucket.count >= RATE_LIMIT_PER_MINUTE {
            return false;
        }
        bucket.count += 1;
        true
    }
}

#[derive(Debug, Deserialize)]
pub struct DiagnosticsIngestBody {
    pub device_id: String,
    pub events: Vec<DiagnosticsEventIn>,
}

#[derive(Debug, Deserialize)]
pub struct DiagnosticsEventIn {
    pub event_type: String,
    pub severity: String,
    pub app_version: Option<String>,
    pub os: Option<String>,
    pub arch: Option<String>,
    pub channel: Option<String>,
    pub phase: Option<String>,
    pub context: Option<Value>,
    #[serde(default)]
    pub ts: Option<u64>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct DiagnosticsEventRow {
    pub id: Uuid,
    pub device_id: String,
    pub account_id: Option<Uuid>,
    pub org_id: Option<Uuid>,
    pub event_type: String,
    pub severity: String,
    pub app_version: Option<String>,
    pub os: Option<String>,
    pub arch: Option<String>,
    pub channel: Option<String>,
    pub phase: Option<String>,
    pub context: Value,
    pub created_at: DateTime<Utc>,
}

pub async fn ingest(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<DiagnosticsIngestBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let device_id = clean_device_id(&body.device_id)?;
    let rate_key = rate_key(&device_id, &headers);
    if !state.diagnostics_rate_limit.allow(&rate_key).await {
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({"error": "rate limited"})),
        ));
    }

    if body.events.is_empty() {
        return Ok(Json(json!({ "ok": true, "accepted": 0 })));
    }
    if body.events.len() > MAX_EVENTS_PER_REQUEST {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(json!({"error": "too many events in batch"})),
        ));
    }

    let mut accepted = 0usize;
    for event in body.events {
        if !event_is_safe(&event) {
            continue;
        }
        let event_type = clean_field(&event.event_type, MAX_EVENT_TYPE_LEN, "event_type")?;
        let severity = normalize_severity(&event.severity);
        let context = sanitize_context(event.context.unwrap_or_else(|| json!({})));

        sqlx::query(
            "INSERT INTO diagnostics_events (
                device_id, event_type, severity, app_version, os, arch, channel, phase, context
             ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)",
        )
        .bind(&device_id)
        .bind(event_type)
        .bind(severity)
        .bind(trim_optional(event.app_version, 40))
        .bind(trim_optional(event.os, 40))
        .bind(trim_optional(event.arch, 40))
        .bind(trim_optional(event.channel, 40))
        .bind(trim_optional(event.phase, MAX_PHASE_LEN))
        .bind(context)
        .execute(&state.db)
        .await
        .map_err(db_err)?;

        accepted += 1;
    }

    Ok(Json(json!({ "ok": true, "accepted": accepted })))
}

pub async fn list(
    State(state): State<AppState>,
    _user: AuthUser,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let rows: Vec<DiagnosticsEventRow> = sqlx::query_as(
        "SELECT id, device_id, account_id, org_id, event_type, severity,
                app_version, os, arch, channel, phase, context, created_at
           FROM diagnostics_events
          ORDER BY created_at DESC
          LIMIT 500",
    )
    .fetch_all(&state.db)
    .await
    .map_err(db_err)?;

    Ok(Json(json!({ "events": rows })))
}

fn event_is_safe(event: &DiagnosticsEventIn) -> bool {
    let ctx = event.context.as_ref().unwrap_or(&Value::Null);
    let serialized = ctx.to_string();
    if serialized.len() > 16_384 {
        return false;
    }
    !contains_blocked_key(ctx)
}

fn contains_blocked_key(value: &Value) -> bool {
    const BLOCKED: &[&str] = &[
        "transcript",
        "polished",
        "raw_transcript",
        "enriched_transcript",
        "audio",
        "api_key",
        "secret",
        "password",
        "token",
        "authorization",
        "user_text",
        "user_kept",
        "ai_output",
    ];
    match value {
        Value::Object(map) => map.iter().any(|(key, child)| {
            let lower = key.to_ascii_lowercase();
            BLOCKED.iter().any(|b| lower.contains(b)) || contains_blocked_key(child)
        }),
        Value::Array(items) => items.iter().any(contains_blocked_key),
        _ => false,
    }
}

fn sanitize_context(mut value: Value) -> Value {
    if let Value::Object(map) = &mut value {
        map.retain(|key, _| {
            let lower = key.to_ascii_lowercase();
            !lower.contains("transcript")
                && !lower.contains("polished")
                && !lower.contains("audio")
                && !lower.contains("secret")
                && !lower.contains("password")
                && !lower.contains("api_key")
                && !lower.contains("token")
                && !lower.contains("authorization")
        });
    }
    value
}

fn rate_key(device_id: &str, headers: &HeaderMap) -> String {
    match client_ip_hint(headers) {
        Some(ip) => format!("{device_id}:{ip}"),
        None => device_id.to_string(),
    }
}

fn client_ip_hint(headers: &HeaderMap) -> Option<String> {
    for name in ["cf-connecting-ip", "x-real-ip", "x-forwarded-for"] {
        let Some(value) = headers.get(name).and_then(|v| v.to_str().ok()) else {
            continue;
        };
        let Some(first) = value.split(',').next().map(str::trim) else {
            continue;
        };
        if !first.is_empty()
            && first.len() <= 64
            && first
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | ':' | '-'))
        {
            return Some(first.to_string());
        }
    }
    None
}

fn clean_device_id(raw: &str) -> Result<String, (StatusCode, Json<Value>)> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_DEVICE_ID_LEN {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"error": "invalid device_id"})),
        ));
    }
    Ok(trimmed.to_string())
}

fn clean_field(raw: &str, max: usize, field: &str) -> Result<String, (StatusCode, Json<Value>)> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > max {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"error": format!("invalid {field}")})),
        ));
    }
    Ok(trimmed.to_string())
}

fn trim_optional(raw: Option<String>, max: usize) -> Option<String> {
    raw.map(|s| s.trim().chars().take(max).collect::<String>())
        .filter(|s: &String| !s.is_empty())
}

fn normalize_severity(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "info" => "info",
        "warning" | "warn" => "warning",
        "fatal" => "fatal",
        _ => "error",
    }
    .to_string()
}

fn db_err(err: sqlx::Error) -> (StatusCode, Json<Value>) {
    tracing::error!("[diagnostics] db error: {err}");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": "database error"})),
    )
}
