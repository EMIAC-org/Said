//! Database helpers — thin wrapper around the sqlx PgPool.
//!
//! Migrations are embedded and executed on every startup. All DDL uses
//! `IF NOT EXISTS`, so the script is safe to re-run. We avoid the sqlx `migrate`
//! feature (which pulls sqlx-sqlite and conflicts with rusqlite elsewhere in the
//! repo) and run the SQL directly under a Postgres advisory lock.

use sqlx::PgPool;
use tracing::info;

pub type Db = PgPool;

const MIGRATIONS: &[&str] = &[include_str!("../migrations/0001_init.sql")];

/// Connect to Postgres and apply the schema.
pub async fn connect(database_url: &str) -> Result<Db, sqlx::Error> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(10)
        .connect(database_url)
        .await?;

    info!("[store] applying schema");
    sqlx::query("SELECT pg_advisory_lock(hashtext('airnote_mobile_gateway_migrations'))")
        .execute(&pool)
        .await?;
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
        sqlx::query("SELECT pg_advisory_unlock(hashtext('airnote_mobile_gateway_migrations'))")
            .execute(&pool)
            .await;
    migration_result?;
    unlock_result?;
    info!("[store] schema OK");

    Ok(pool)
}
