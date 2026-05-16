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
];

/// Connect to Postgres and apply the schema.
pub async fn connect(database_url: &str) -> Result<Db, sqlx::Error> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(10)
        .connect(database_url)
        .await?;

    info!("[store] applying schema");
    // Run each migration file sequentially; split on statement boundaries
    for migration in MIGRATIONS {
        for stmt in migration.split(';') {
            let trimmed = stmt.trim();
            if !trimmed.is_empty() {
                sqlx::query(trimmed).execute(&pool).await?;
            }
        }
    }
    info!("[store] schema OK");

    Ok(pool)
}
