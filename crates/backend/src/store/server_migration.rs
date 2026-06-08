//! Persistent state for the first-launch local → server data migration.

use rusqlite::params;
use serde::{Deserialize, Serialize};

use super::DbPool;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MigrationStatus {
    NotStarted,
    Running,
    Partial,
    Completed,
    Failed,
}

impl MigrationStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NotStarted => "not_started",
            Self::Running => "running",
            Self::Partial => "partial",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "running" => Self::Running,
            "partial" => Self::Partial,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            _ => Self::NotStarted,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerMigrationState {
    pub user_id: String,
    pub server_account_id: String,
    pub migration_version: i64,
    pub status: MigrationStatus,
    pub started_at_ms: Option<i64>,
    pub completed_at_ms: Option<i64>,
    pub last_attempt_at_ms: Option<i64>,
    pub last_error: Option<String>,
    pub uploaded_history_count: i64,
    pub uploaded_vocab_count: i64,
    pub uploaded_alias_count: i64,
    pub uploaded_email_count: i64,
    pub uploaded_credentials_count: i64,
}

pub fn get_state(
    pool: &DbPool,
    user_id: &str,
    server_account_id: &str,
) -> Option<ServerMigrationState> {
    let conn = pool.get().ok()?;
    conn.query_row(
        "SELECT user_id, server_account_id, migration_version, status,
                started_at_ms, completed_at_ms, last_attempt_at_ms, last_error,
                uploaded_history_count, uploaded_vocab_count, uploaded_alias_count,
                uploaded_email_count, uploaded_credentials_count
           FROM server_migration_state
          WHERE user_id = ?1 AND server_account_id = ?2
          ORDER BY migration_version DESC LIMIT 1",
        params![user_id, server_account_id],
        |row| {
            Ok(ServerMigrationState {
                user_id: row.get(0)?,
                server_account_id: row.get(1)?,
                migration_version: row.get(2)?,
                status: MigrationStatus::parse(&row.get::<_, String>(3)?),
                started_at_ms: row.get(4)?,
                completed_at_ms: row.get(5)?,
                last_attempt_at_ms: row.get(6)?,
                last_error: row.get(7)?,
                uploaded_history_count: row.get(8)?,
                uploaded_vocab_count: row.get(9)?,
                uploaded_alias_count: row.get(10)?,
                uploaded_email_count: row.get(11)?,
                uploaded_credentials_count: row.get(12)?,
            })
        },
    )
    .ok()
}

pub fn ensure_row(pool: &DbPool, user_id: &str, server_account_id: &str, version: i64) {
    let conn = match pool.get() {
        Ok(c) => c,
        Err(_) => return,
    };
    let _ = conn.execute(
        "INSERT OR IGNORE INTO server_migration_state
             (user_id, server_account_id, migration_version, status)
         VALUES (?1, ?2, ?3, 'not_started')",
        params![user_id, server_account_id, version],
    );
}

pub fn set_status(
    pool: &DbPool,
    user_id: &str,
    server_account_id: &str,
    version: i64,
    status: MigrationStatus,
    error: Option<&str>,
) {
    let conn = match pool.get() {
        Ok(c) => c,
        Err(_) => return,
    };
    let now_ms = crate::store::now_ms();

    let started_update = if status == MigrationStatus::Running {
        "started_at_ms = ?5,"
    } else {
        ""
    };
    let completed_update = if status == MigrationStatus::Completed {
        "completed_at_ms = ?5,"
    } else {
        ""
    };
    let _ = started_update; // suppress unused warning
    let _ = completed_update;

    let _ = conn.execute(
        "UPDATE server_migration_state
            SET status = ?4,
                last_attempt_at_ms = ?5,
                last_error = ?6,
                started_at_ms = CASE WHEN ?4 = 'running' AND started_at_ms IS NULL
                                     THEN ?5 ELSE started_at_ms END,
                completed_at_ms = CASE WHEN ?4 = 'completed' THEN ?5 ELSE completed_at_ms END
          WHERE user_id = ?1 AND server_account_id = ?2 AND migration_version = ?3",
        params![
            user_id,
            server_account_id,
            version,
            status.as_str(),
            now_ms,
            error
        ],
    );
}

pub fn update_counts(
    pool: &DbPool,
    user_id: &str,
    server_account_id: &str,
    version: i64,
    history: i64,
    vocab: i64,
    aliases: i64,
    emails: i64,
    credentials: i64,
) {
    let conn = match pool.get() {
        Ok(c) => c,
        Err(_) => return,
    };
    let _ = conn.execute(
        "UPDATE server_migration_state
            SET uploaded_history_count = uploaded_history_count + ?4,
                uploaded_vocab_count   = uploaded_vocab_count + ?5,
                uploaded_alias_count   = uploaded_alias_count + ?6,
                uploaded_email_count   = uploaded_email_count + ?7,
                uploaded_credentials_count = uploaded_credentials_count + ?8
          WHERE user_id = ?1 AND server_account_id = ?2 AND migration_version = ?3",
        params![
            user_id,
            server_account_id,
            version,
            history,
            vocab,
            aliases,
            emails,
            credentials
        ],
    );
}
