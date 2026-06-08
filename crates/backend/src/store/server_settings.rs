//! Local mirror of server-side runtime settings.

use rusqlite::params;
use serde::{Deserialize, Serialize};

use super::{DbPool, now_ms};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedServerSettings {
    pub user_id: String,
    pub server_account_id: String,
    pub settings_json: String,
    pub server_version: i64,
    pub last_synced_at_ms: Option<i64>,
    pub last_error: Option<String>,
}

pub fn get(pool: &DbPool, user_id: &str, server_account_id: &str) -> Option<CachedServerSettings> {
    let conn = pool.get().ok()?;
    conn.query_row(
        "SELECT user_id, server_account_id, settings_json, server_version,
                last_synced_at_ms, last_error
           FROM server_settings_state
          WHERE user_id = ?1 AND server_account_id = ?2",
        params![user_id, server_account_id],
        |row| {
            Ok(CachedServerSettings {
                user_id: row.get(0)?,
                server_account_id: row.get(1)?,
                settings_json: row.get(2)?,
                server_version: row.get(3)?,
                last_synced_at_ms: row.get(4)?,
                last_error: row.get(5)?,
            })
        },
    )
    .ok()
}

pub fn put(
    pool: &DbPool,
    user_id: &str,
    server_account_id: &str,
    settings_json: &str,
    server_version: i64,
) {
    let conn = match pool.get() {
        Ok(c) => c,
        Err(_) => return,
    };
    let now = now_ms();
    let _ = conn.execute(
        "INSERT INTO server_settings_state
                (user_id, server_account_id, settings_json, server_version, last_synced_at_ms, last_error)
         VALUES (?1, ?2, ?3, ?4, ?5, NULL)
         ON CONFLICT (user_id, server_account_id)
         DO UPDATE SET settings_json     = excluded.settings_json,
                       server_version    = excluded.server_version,
                       last_synced_at_ms = excluded.last_synced_at_ms,
                       last_error        = NULL",
        params![user_id, server_account_id, settings_json, server_version, now],
    );
}

pub fn set_error(pool: &DbPool, user_id: &str, server_account_id: &str, error: &str) {
    let conn = match pool.get() {
        Ok(c) => c,
        Err(_) => return,
    };
    let _ = conn.execute(
        "INSERT INTO server_settings_state (user_id, server_account_id, last_error)
         VALUES (?1, ?2, ?3)
         ON CONFLICT (user_id, server_account_id)
         DO UPDATE SET last_error = excluded.last_error",
        params![user_id, server_account_id, error],
    );
}
