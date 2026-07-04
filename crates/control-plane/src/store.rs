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
    include_str!("../migrations/010_company_vocab.sql"),
    include_str!("../migrations/011_email_auth_source.sql"),
    include_str!("../migrations/012_diagnostics_events.sql"),
    include_str!("../migrations/013_runtime_gateway.sql"),
    include_str!("../migrations/014_runtime_history.sql"),
    include_str!("../migrations/015_runtime_settings.sql"),
    include_str!("../migrations/016_multi_org.sql"),
    include_str!("../migrations/017_telemetry.sql"),
    include_str!("../migrations/018_telemetry_stt.sql"),
    include_str!("../migrations/019_memory_hygiene.sql"),
    include_str!("../migrations/020_polish_model_deepseek.sql"),
    include_str!("../migrations/021_remove_deepseek_polish_model.sql"),
    include_str!("../migrations/022_runtime_beta_providers.sql"),
    include_str!("../migrations/023_runtime_user_profiles.sql"),
    include_str!("../migrations/024_profile_learn_jobs.sql"),
    include_str!("../migrations/025_default_gpt_oss_20b.sql"),
    include_str!("../migrations/026_default_cerebras_gpt_oss_120b.sql"),
    include_str!("../migrations/027_lock_cerebras_polish_defaults.sql"),
    include_str!("../migrations/028_profile_hitl_review.sql"),
    include_str!("../migrations/029_runtime_alias_learn_events.sql"),
    include_str!("../migrations/030_runtime_prompt_profile_latest.sql"),
    include_str!("../migrations/031_dictation_trace.sql"),
];

/// Connect to Postgres and apply the schema.
pub async fn connect(database_url: &str) -> Result<Db, sqlx::Error> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(10)
        .connect(database_url)
        .await?;

    info!("[store] applying schema");
    let mut conn = pool.acquire().await?;
    sqlx::query("SELECT pg_advisory_lock(hashtext('said_control_plane_migrations'))")
        .execute(&mut *conn)
        .await?;
    // Run each migration file sequentially; split on statement boundaries.
    // Strip full-line comments first because this lightweight runner splits on
    // semicolons and comments can contain prose punctuation.
    let migration_result = async {
        for migration in MIGRATIONS {
            let sql = strip_full_line_sql_comments(migration);
            for stmt in sql.split(';') {
                let trimmed = stmt.trim();
                if !trimmed.is_empty() {
                    sqlx::query(trimmed).execute(&mut *conn).await?;
                }
            }
        }
        Ok::<(), sqlx::Error>(())
    }
    .await;
    let unlock_result =
        sqlx::query("SELECT pg_advisory_unlock(hashtext('said_control_plane_migrations'))")
            .execute(&mut *conn)
            .await;
    migration_result?;
    unlock_result?;
    info!("[store] schema OK");

    Ok(pool)
}

fn strip_full_line_sql_comments(sql: &str) -> String {
    sql.lines()
        .filter(|line| !line.trim_start().starts_with("--"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    #[test]
    fn strips_full_line_comment_semicolons_before_split() {
        let sql = super::strip_full_line_sql_comments("-- comment; prose\nSELECT 1;");
        assert_eq!(sql, "SELECT 1;");
    }
}
