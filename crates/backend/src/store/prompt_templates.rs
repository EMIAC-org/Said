use rusqlite::{OptionalExtension, params};
use serde::Serialize;

use super::{DbPool, now_ms};

#[derive(Debug, Clone, Serialize)]
pub struct PromptTemplate {
    pub kind: String,
    pub title: String,
    pub base_version: String,
    pub active_body: String,
    pub draft_body: Option<String>,
    pub updated_at: i64,
    pub applied_at: Option<i64>,
}

#[derive(Debug, Clone, Copy)]
pub struct DefaultPrompt<'a> {
    pub kind: &'a str,
    pub title: &'a str,
    pub base_version: &'a str,
    pub body: &'a str,
}

pub fn get_or_seed(
    pool: &DbPool,
    user_id: &str,
    default_prompt: DefaultPrompt<'_>,
) -> Option<PromptTemplate> {
    if let Some(existing) = get(pool, user_id, default_prompt.kind) {
        if let Some(upgraded) = maybe_upgrade_seed_default(pool, user_id, default_prompt, &existing)
        {
            return Some(upgraded);
        }
        return Some(existing);
    }

    let conn = pool.get().ok()?;
    let now = now_ms();
    conn.execute(
        "INSERT OR IGNORE INTO prompt_templates
         (user_id, kind, title, base_version, active_body, draft_body, updated_at, applied_at)
         VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?6)",
        params![
            user_id,
            default_prompt.kind,
            default_prompt.title,
            default_prompt.base_version,
            default_prompt.body,
            now
        ],
    )
    .ok()?;
    record_event(
        &conn,
        user_id,
        default_prompt.kind,
        "seed",
        default_prompt.body,
        now,
    );
    get(pool, user_id, default_prompt.kind)
}

fn maybe_upgrade_seed_default(
    pool: &DbPool,
    user_id: &str,
    default_prompt: DefaultPrompt<'_>,
    existing: &PromptTemplate,
) -> Option<PromptTemplate> {
    if existing.base_version == default_prompt.base_version {
        return None;
    }

    let conn = pool.get().ok()?;
    let non_seed_events: i64 = conn
        .query_row(
            "SELECT COUNT(*)
             FROM prompt_template_events
             WHERE user_id = ?1 AND kind = ?2 AND event_type NOT IN ('seed', 'upgrade_default')",
            params![user_id, default_prompt.kind],
            |row| row.get(0),
        )
        .unwrap_or(1);
    if non_seed_events != 0 {
        return None;
    }

    let now = now_ms();
    conn.execute(
        "UPDATE prompt_templates
         SET title = ?1, base_version = ?2, active_body = ?3, draft_body = NULL, updated_at = ?4, applied_at = ?4
         WHERE user_id = ?5 AND kind = ?6",
        params![
            default_prompt.title,
            default_prompt.base_version,
            default_prompt.body,
            now,
            user_id,
            default_prompt.kind
        ],
    )
    .ok()?;
    record_event(
        &conn,
        user_id,
        default_prompt.kind,
        "upgrade_default",
        default_prompt.body,
        now,
    );
    get(pool, user_id, default_prompt.kind)
}

pub fn get(pool: &DbPool, user_id: &str, kind: &str) -> Option<PromptTemplate> {
    let conn = pool.get().ok()?;
    conn.query_row(
        "SELECT kind, title, base_version, active_body, draft_body, updated_at, applied_at
         FROM prompt_templates
         WHERE user_id = ?1 AND kind = ?2",
        params![user_id, kind],
        |row| {
            Ok(PromptTemplate {
                kind: row.get(0)?,
                title: row.get(1)?,
                base_version: row.get(2)?,
                active_body: row.get(3)?,
                draft_body: row.get(4)?,
                updated_at: row.get(5)?,
                applied_at: row.get(6)?,
            })
        },
    )
    .optional()
    .ok()
    .flatten()
}

pub fn active_body_or_default(
    pool: &DbPool,
    user_id: &str,
    default_prompt: DefaultPrompt<'_>,
) -> String {
    get_or_seed(pool, user_id, default_prompt.clone())
        .map(|template| {
            if template.active_body.trim().is_empty() {
                default_prompt.body.to_string()
            } else {
                template.active_body
            }
        })
        .unwrap_or_else(|| default_prompt.body.to_string())
}

pub fn save_draft(
    pool: &DbPool,
    user_id: &str,
    default_prompt: DefaultPrompt<'_>,
    draft_body: &str,
) -> Option<PromptTemplate> {
    let _ = get_or_seed(pool, user_id, default_prompt);
    let conn = pool.get().ok()?;
    let now = now_ms();
    conn.execute(
        "UPDATE prompt_templates
         SET draft_body = ?1, updated_at = ?2
         WHERE user_id = ?3 AND kind = ?4",
        params![draft_body, now, user_id, default_prompt.kind],
    )
    .ok()?;
    record_event(
        &conn,
        user_id,
        default_prompt.kind,
        "draft",
        draft_body,
        now,
    );
    get(pool, user_id, default_prompt.kind)
}

pub fn apply_draft(
    pool: &DbPool,
    user_id: &str,
    default_prompt: DefaultPrompt<'_>,
) -> Option<PromptTemplate> {
    let current = get_or_seed(pool, user_id, default_prompt)?;
    let next_body = current
        .draft_body
        .as_deref()
        .filter(|body| !body.trim().is_empty())
        .unwrap_or(&current.active_body)
        .to_string();
    let conn = pool.get().ok()?;
    let now = now_ms();
    conn.execute(
        "UPDATE prompt_templates
         SET active_body = ?1, draft_body = NULL, updated_at = ?2, applied_at = ?2
         WHERE user_id = ?3 AND kind = ?4",
        params![next_body, now, user_id, default_prompt.kind],
    )
    .ok()?;
    record_event(
        &conn,
        user_id,
        default_prompt.kind,
        "apply",
        &next_body,
        now,
    );
    get(pool, user_id, default_prompt.kind)
}

pub fn reset_to_default(
    pool: &DbPool,
    user_id: &str,
    default_prompt: DefaultPrompt<'_>,
) -> Option<PromptTemplate> {
    let _ = get_or_seed(pool, user_id, default_prompt.clone());
    let conn = pool.get().ok()?;
    let now = now_ms();
    conn.execute(
        "UPDATE prompt_templates
         SET title = ?1, base_version = ?2, active_body = ?3, draft_body = NULL, updated_at = ?4, applied_at = ?4
         WHERE user_id = ?5 AND kind = ?6",
        params![
            default_prompt.title,
            default_prompt.base_version,
            default_prompt.body,
            now,
            user_id,
            default_prompt.kind
        ],
    )
    .ok()?;
    record_event(
        &conn,
        user_id,
        default_prompt.kind,
        "reset",
        default_prompt.body,
        now,
    );
    get(pool, user_id, default_prompt.kind)
}

fn record_event(
    conn: &rusqlite::Connection,
    user_id: &str,
    kind: &str,
    event_type: &str,
    body_snapshot: &str,
    created_at: i64,
) {
    let id = uuid::Uuid::new_v4().to_string();
    let _ = conn.execute(
        "INSERT INTO prompt_template_events
         (id, user_id, kind, event_type, body_snapshot, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, user_id, kind, event_type, body_snapshot, created_at],
    );
}
