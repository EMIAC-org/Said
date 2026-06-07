//! Small shared helpers: validation, redaction, and the common API result type.

use axum::{Json, http::StatusCode};
use serde_json::{Value, json};

pub type ApiResult<T> = Result<T, (StatusCode, Json<Value>)>;

pub const MAX_DEVICE_ID_LEN: usize = 160;
pub const MAX_CLIENT_REQUEST_ID_LEN: usize = 120;
pub const MAX_EVENT_ID_LEN: usize = 120;
pub const MAX_EVENT_TYPE_LEN: usize = 120;
pub const MAX_LABEL_LEN: usize = 120;
pub const MAX_FIELD_HINT_LEN: usize = 80;
pub const MAX_TEXT_CONTEXT_LEN: usize = 2_000;
pub const MAX_RECORDING_SECONDS: i32 = 60;
pub const SESSION_TTL_MINUTES: i64 = 15;

/// Supported value sets, kept here so every route normalizes identically.
pub const PLATFORMS: &[&str] = &["ios", "android"];
pub const SURFACES: &[&str] = &[
    "ios_keyboard",
    "ios_action_button",
    "android_keyboard",
    "android_bubble",
];
pub const LANGUAGES: &[&str] = &["auto", "en", "hi", "hinglish"];
pub const STYLES: &[&str] = &["direct", "work", "casual", "email", "notes"];

pub fn db_err(err: sqlx::Error) -> (StatusCode, Json<Value>) {
    tracing::debug!("[mobile-gateway] database error: {err}");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": "database error"})),
    )
}

pub fn bad_request(message: &str) -> (StatusCode, Json<Value>) {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(json!({ "error": message })),
    )
}

pub fn clean_required(raw: &str, max_len: usize, name: &str) -> ApiResult<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > max_len {
        return Err(bad_request(&format!(
            "{name} required and must be <= {max_len} chars"
        )));
    }
    Ok(trimmed.to_string())
}

pub fn clean_optional(raw: &str, max_len: usize) -> ApiResult<String> {
    let trimmed = raw.trim();
    if trimmed.len() > max_len {
        return Err(bad_request(&format!("field must be <= {max_len} chars")));
    }
    Ok(trimmed.to_string())
}

pub fn trim_optional(raw: Option<String>, max_len: usize) -> Option<String> {
    raw.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.chars().take(max_len).collect())
        }
    })
}

pub fn normalize_choice(raw: Option<&str>, allowed: &[&str], default: &str) -> String {
    let Some(raw) = raw else {
        return default.to_string();
    };
    let normalized = raw.trim().to_ascii_lowercase();
    if allowed.iter().any(|item| *item == normalized) {
        normalized
    } else {
        default.to_string()
    }
}

/// Count characters in `raw`, capped — used to record *how much* cursor context
/// the client sent without ever storing the text itself.
pub fn bounded_char_count(raw: &str) -> usize {
    raw.chars()
        .take(MAX_TEXT_CONTEXT_LEN + 1)
        .count()
        .min(MAX_TEXT_CONTEXT_LEN)
}

/// Recursively drop any key that could carry raw user content or secrets, and
/// clamp string lengths. Defence-in-depth for client-supplied event context.
pub fn sanitize_context(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, child) in map {
                if is_blocked_key(&key) {
                    continue;
                }
                out.insert(key, sanitize_context(child));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.into_iter().map(sanitize_context).collect()),
        Value::String(text) => Value::String(text.chars().take(500).collect()),
        other => other,
    }
}

fn is_blocked_key(key: &str) -> bool {
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
        "before_text",
        "after_text",
        "selected_text",
    ];
    let lower = key.to_ascii_lowercase();
    BLOCKED.iter().any(|blocked| lower.contains(blocked))
}
