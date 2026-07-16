//! Database helpers — thin wrapper around the sqlx PgPool.
//!
//! We apply embedded migrations with a small Postgres-side ledger. This avoids
//! the `sqlx::migrate!` macro because enabling sqlx migrations pulls in
//! sqlx-sqlite, which conflicts with rusqlite in this workspace.

use std::collections::HashSet;

use sqlx::{PgPool, Row};
use tracing::info;

pub type Db = PgPool;

struct Migration {
    id: &'static str,
    sql: &'static str,
}

/// Existing dev/prod databases were created before the ledger existed. On first
/// ledger startup, mark these historical migrations as already applied, then run
/// 035+ normally. This prevents old rerunnable constraint migrations from
/// rejecting current rows while still applying the Gemma 4 repair.
const BASELINE_EXISTING_SCHEMA_THROUGH: &str = "034_app_bucket_user_source";

const MIGRATIONS: &[Migration] = &[
    Migration {
        id: "001_initial",
        sql: include_str!("../migrations/001_initial.sql"),
    },
    Migration {
        id: "002_enterprise",
        sql: include_str!("../migrations/002_enterprise.sql"),
    },
    Migration {
        id: "003_slots_roles",
        sql: include_str!("../migrations/003_slots_roles.sql"),
    },
    Migration {
        id: "004_openai_account",
        sql: include_str!("../migrations/004_openai_account.sql"),
    },
    Migration {
        id: "005_pre_meeting",
        sql: include_str!("../migrations/005_pre_meeting.sql"),
    },
    Migration {
        id: "006_lark_calendar_events",
        sql: include_str!("../migrations/006_lark_calendar_events.sql"),
    },
    Migration {
        id: "007_guest_links",
        sql: include_str!("../migrations/007_guest_links.sql"),
    },
    Migration {
        id: "008_desktop_clients",
        sql: include_str!("../migrations/008_desktop_clients.sql"),
    },
    Migration {
        id: "009_bug_reports",
        sql: include_str!("../migrations/009_bug_reports.sql"),
    },
    Migration {
        id: "010_company_vocab",
        sql: include_str!("../migrations/010_company_vocab.sql"),
    },
    Migration {
        id: "011_email_auth_source",
        sql: include_str!("../migrations/011_email_auth_source.sql"),
    },
    Migration {
        id: "012_diagnostics_events",
        sql: include_str!("../migrations/012_diagnostics_events.sql"),
    },
    Migration {
        id: "013_runtime_gateway",
        sql: include_str!("../migrations/013_runtime_gateway.sql"),
    },
    Migration {
        id: "014_runtime_history",
        sql: include_str!("../migrations/014_runtime_history.sql"),
    },
    Migration {
        id: "015_runtime_settings",
        sql: include_str!("../migrations/015_runtime_settings.sql"),
    },
    Migration {
        id: "016_multi_org",
        sql: include_str!("../migrations/016_multi_org.sql"),
    },
    Migration {
        id: "017_telemetry",
        sql: include_str!("../migrations/017_telemetry.sql"),
    },
    Migration {
        id: "018_telemetry_stt",
        sql: include_str!("../migrations/018_telemetry_stt.sql"),
    },
    Migration {
        id: "019_memory_hygiene",
        sql: include_str!("../migrations/019_memory_hygiene.sql"),
    },
    Migration {
        id: "020_polish_model_deepseek",
        sql: include_str!("../migrations/020_polish_model_deepseek.sql"),
    },
    Migration {
        id: "021_remove_deepseek_polish_model",
        sql: include_str!("../migrations/021_remove_deepseek_polish_model.sql"),
    },
    Migration {
        id: "022_runtime_beta_providers",
        sql: include_str!("../migrations/022_runtime_beta_providers.sql"),
    },
    Migration {
        id: "023_runtime_user_profiles",
        sql: include_str!("../migrations/023_runtime_user_profiles.sql"),
    },
    Migration {
        id: "024_profile_learn_jobs",
        sql: include_str!("../migrations/024_profile_learn_jobs.sql"),
    },
    Migration {
        id: "025_default_gpt_oss_20b",
        sql: include_str!("../migrations/025_default_gpt_oss_20b.sql"),
    },
    Migration {
        id: "026_default_cerebras_gpt_oss_120b",
        sql: include_str!("../migrations/026_default_cerebras_gpt_oss_120b.sql"),
    },
    Migration {
        id: "027_lock_cerebras_polish_defaults",
        sql: include_str!("../migrations/027_lock_cerebras_polish_defaults.sql"),
    },
    Migration {
        id: "028_profile_hitl_review",
        sql: include_str!("../migrations/028_profile_hitl_review.sql"),
    },
    Migration {
        id: "029_runtime_alias_learn_events",
        sql: include_str!("../migrations/029_runtime_alias_learn_events.sql"),
    },
    Migration {
        id: "030_runtime_prompt_profile_latest",
        sql: include_str!("../migrations/030_runtime_prompt_profile_latest.sql"),
    },
    Migration {
        id: "031_dictation_trace",
        sql: include_str!("../migrations/031_dictation_trace.sql"),
    },
    Migration {
        id: "032_app_buckets",
        sql: include_str!("../migrations/032_app_buckets.sql"),
    },
    Migration {
        id: "033_profile_batch_jobs",
        sql: include_str!("../migrations/033_profile_batch_jobs.sql"),
    },
    Migration {
        id: "034_app_bucket_user_source",
        sql: include_str!("../migrations/034_app_bucket_user_source.sql"),
    },
    Migration {
        id: "035_force_cerebras_gemma_4",
        sql: include_str!("../migrations/035_force_cerebras_gemma_4.sql"),
    },
    Migration {
        id: "036_openrouter_gemma_4_nitro",
        sql: include_str!("../migrations/036_openrouter_gemma_4_nitro.sql"),
    },
    Migration {
        id: "037_together_gemma_4",
        sql: include_str!("../migrations/037_together_gemma_4.sql"),
    },
    Migration {
        id: "038_openrouter_gemma_4_nitro",
        sql: include_str!("../migrations/038_openrouter_gemma_4_nitro.sql"),
    },
    Migration {
        id: "039_telemetry_model_costs",
        sql: include_str!("../migrations/039_telemetry_model_costs.sql"),
    },
    Migration {
        id: "040_backfill_late_telemetry_costs",
        sql: include_str!("../migrations/040_backfill_late_telemetry_costs.sql"),
    },
    Migration {
        id: "041_deepinfra_gemma_4_26b_a4b",
        sql: include_str!("../migrations/041_deepinfra_gemma_4_26b_a4b.sql"),
    },
    Migration {
        id: "042_meeting_provider_usage",
        sql: include_str!("../migrations/042_meeting_provider_usage.sql"),
    },
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
    let migration_result = async {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS airnote_control_plane_migrations (
                id         TEXT PRIMARY KEY,
                applied_at TIMESTAMPTZ NOT NULL DEFAULT now()
            )",
        )
        .execute(&mut *conn)
        .await?;

        let initialized =
            sqlx::query("SELECT to_regclass('public.accounts') IS NOT NULL AS initialized")
                .fetch_one(&mut *conn)
                .await?
                .try_get::<bool, _>("initialized")?;

        let rows = sqlx::query("SELECT id FROM airnote_control_plane_migrations")
            .fetch_all(&mut *conn)
            .await?;
        let mut applied = rows
            .into_iter()
            .filter_map(|row| row.try_get::<String, _>("id").ok())
            .collect::<HashSet<_>>();

        if initialized && applied.is_empty() {
            info!(
                "[store] baselining existing schema through {}",
                BASELINE_EXISTING_SCHEMA_THROUGH
            );
            for migration in MIGRATIONS {
                sqlx::query(
                    "INSERT INTO airnote_control_plane_migrations (id)
                     VALUES ($1)
                     ON CONFLICT (id) DO NOTHING",
                )
                .bind(migration.id)
                .execute(&mut *conn)
                .await?;
                applied.insert(migration.id.to_string());
                if migration.id == BASELINE_EXISTING_SCHEMA_THROUGH {
                    break;
                }
            }
        }

        for migration in MIGRATIONS {
            if applied.contains(migration.id) {
                continue;
            }
            info!("[store] applying migration {}", migration.id);
            let sql = strip_full_line_sql_comments(migration.sql);
            for stmt in sql.split(';') {
                let trimmed = stmt.trim();
                if !trimmed.is_empty() {
                    sqlx::query(trimmed).execute(&mut *conn).await?;
                }
            }
            sqlx::query(
                "INSERT INTO airnote_control_plane_migrations (id)
                 VALUES ($1)
                 ON CONFLICT (id) DO NOTHING",
            )
            .bind(migration.id)
            .execute(&mut *conn)
            .await?;
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
