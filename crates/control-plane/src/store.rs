//! Database helpers — thin wrapper around the sqlx PgPool.
//!
//! We apply migrations by running the embedded SQL directly (all DDL uses
//! `IF NOT EXISTS`, so the script is safe to re-run on every startup).
//! This avoids the sqlx `migrate` feature which transitively pulls in
//! sqlx-sqlite and conflicts with rusqlite in the same workspace.

use sqlx::PgPool;
use tracing::info;

pub type Db = PgPool;

/// Embedded migration SQL — executed on every startup (idempotent).
const MIGRATIONS: &[&str] = &[
    include_str!("../migrations/001_initial.sql"),
    include_str!("../migrations/002_enterprise.sql"),
    include_str!("../migrations/003_slots_roles.sql"),
    include_str!("../migrations/004_openai_account.sql"),
    include_str!("../migrations/005_pre_meeting.sql"),
    include_str!("../migrations/006_lark_calendar_events.sql"),
    include_str!("../migrations/007_guest_links.sql"),
    include_str!("../migrations/008_desktop_clients.sql"),
    include_str!("../migrations/009_bug_reports.sql"),
];

/// Connect to Postgres and apply the schema.
pub async fn connect(database_url: &str) -> Result<Db, sqlx::Error> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(10)
        .connect(database_url)
        .await?;

    info!("[store] applying schema");
    sqlx::query("SELECT pg_advisory_lock(hashtext('said_control_plane_migrations'))")
        .execute(&pool)
        .await?;
    // Run each migration file sequentially; split on statement boundaries
    let migration_result = async {
        for migration in MIGRATIONS {
            for stmt in migration.split(';') {
                let trimmed = stmt.trim();
                if !trimmed.is_empty() {
                    sqlx::query(trimmed).execute(&pool).await?;
                }
            }
        }
        Ok::<(), sqlx::Error>(())
    }
    .await;
    let unlock_result =
        sqlx::query("SELECT pg_advisory_unlock(hashtext('said_control_plane_migrations'))")
            .execute(&pool)
            .await;
    migration_result?;
    unlock_result?;
    info!("[store] schema OK");

    Ok(pool)
}
