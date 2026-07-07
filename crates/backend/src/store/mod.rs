use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{Connection, params};
use std::path::PathBuf;
use tracing::{info, warn};

pub mod alias_safety;
pub mod company_vocab;
pub mod corrections;
pub mod email_memory;
pub mod history;
pub mod openai_oauth;
pub mod pending_edits;
pub mod pending_promotions;
pub mod prefs;
pub mod profile_summary;
pub mod server_migration;
pub mod server_settings;
pub mod stt_replacements;
pub mod telemetry;
pub mod tier2_edit_policy;
pub mod tier2_model;
pub mod tier2_policy;
pub mod users;
pub mod vectors;
pub mod vocab_embeddings;
pub mod vocab_fts;
pub mod vocabulary;
pub mod voice_runs;

pub type DbPool = Pool<SqliteConnectionManager>;

const MIGRATION_001: &str = include_str!("migrations/001_initial.sql");
const MIGRATION_002: &str = include_str!("migrations/002_vectors.sql");
const MIGRATION_003: &str = include_str!("migrations/003_output_language.sql");
const MIGRATION_004: &str = include_str!("migrations/004_api_keys.sql");
const MIGRATION_005: &str = include_str!("migrations/005_llm_provider.sql");
const MIGRATION_006: &str = include_str!("migrations/006_openai_oauth.sql");
const MIGRATION_007: &str = include_str!("migrations/007_pending_edits.sql");
const MIGRATION_008: &str = include_str!("migrations/008_recording_audio_id.sql");
const MIGRATION_009: &str = include_str!("migrations/009_word_corrections.sql");
const MIGRATION_010: &str = include_str!("migrations/010_groq_api_key.sql");
const MIGRATION_011: &str = include_str!("migrations/011_embed_dims_256.sql");
const MIGRATION_012: &str = include_str!("migrations/012_vocabulary_and_stt_replacements.sql");
const MIGRATION_013: &str = include_str!("migrations/013_pending_promotions_and_language.sql");
const MIGRATION_014: &str = include_str!("migrations/014_vocabulary_example_context.sql");
const MIGRATION_015: &str = include_str!("migrations/015_vocab_embeddings.sql");
const MIGRATION_016: &str = include_str!("migrations/016_vocab_term_type.sql");
const MIGRATION_017: &str = include_str!("migrations/017_centroid_decay_fts.sql");
const MIGRATION_018: &str = include_str!("migrations/018_vocab_meaning.sql");
const MIGRATION_019: &str = include_str!("migrations/019_background_learning_trust.sql");
const MIGRATION_020: &str = include_str!("migrations/020_record_hotkey.sql");
const MIGRATION_021: &str = include_str!("migrations/021_learning_enabled.sql");
const MIGRATION_022: &str = include_str!("migrations/022_fix_recording_seconds.sql");
const MIGRATION_023: &str = include_str!("migrations/023_enriched_transcript.sql");
const MIGRATION_024: &str = include_str!("migrations/024_prompt_templates.sql");
const MIGRATION_025: &str = include_str!("migrations/025_pending_edits_notified.sql");
const MIGRATION_026: &str = include_str!("migrations/026_cerebras_api_key.sql");
const MIGRATION_027: &str = include_str!("migrations/027_tier2_model_metadata.sql");
const MIGRATION_028: &str = include_str!("migrations/028_tier2_policy_learning.sql");
const MIGRATION_029: &str = include_str!("migrations/029_tier2_edit_policy.sql");
const MIGRATION_030: &str = include_str!("migrations/030_stt_provider.sql");
const MIGRATION_031: &str = include_str!("migrations/031_alias_safety_judgments.sql");
const MIGRATION_032: &str = include_str!("migrations/032_enterprise_server_url.sql");
const MIGRATION_033: &str = include_str!("migrations/033_email_memory.sql");
const MIGRATION_034: &str = include_str!("migrations/034_company_vocab.sql");
const MIGRATION_035: &str = include_str!("migrations/035_server_runtime_probe.sql");
const MIGRATION_036: &str = include_str!("migrations/036_server_audio_runtime_probe.sql");
const MIGRATION_037: &str = include_str!("migrations/037_server_migration_state.sql");
const MIGRATION_038: &str = include_str!("migrations/038_server_settings_state.sql");
const MIGRATION_039: &str = include_str!("migrations/039_enable_server_runtime_for_signed_in.sql");
const MIGRATION_040: &str = include_str!("migrations/040_active_org_id.sql");
const MIGRATION_041: &str = include_str!("migrations/041_telemetry.sql");
const MIGRATION_042: &str = include_str!("migrations/042_telemetry_stt.sql");
const MIGRATION_043: &str = include_str!("migrations/043_sarvam_api_key.sql");
const MIGRATION_044: &str = include_str!("migrations/044_normalize_deepseek_polish_model.sql");
const MIGRATION_045: &str = include_str!("migrations/045_deepinfra_api_key.sql");
const MIGRATION_046: &str = include_str!("migrations/046_force_server_runtime.sql");
const MIGRATION_047: &str = include_str!("migrations/047_default_gpt_oss_20b.sql");
const MIGRATION_048: &str = include_str!("migrations/048_default_cerebras_gpt_oss_120b.sql");
const MIGRATION_049: &str = include_str!("migrations/049_lock_cerebras_polish_defaults.sql");
const MIGRATION_050: &str = include_str!("migrations/050_local_profile_summary.sql");
const MIGRATION_051: &str = include_str!("migrations/051_observability_outbox.sql");
const MIGRATION_052: &str = include_str!("migrations/052_voice_runs.sql");
const MIGRATION_053: &str = include_str!("migrations/053_retire_swift_local_stt.sql");
const MIGRATION_054: &str = include_str!("migrations/054_recording_trace_json.sql");
const MIGRATION_055: &str = include_str!("migrations/055_drop_prompt_templates.sql");
const MIGRATION_056: &str = include_str!("migrations/056_site_visits.sql");

/// Open (or create) the SQLite database at `path`, run pending migrations,
/// and return a connection pool.
pub fn open(path: &PathBuf) -> DbPool {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("failed to create database directory");
    }

    let manager = SqliteConnectionManager::file(path).with_init(|conn| {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;\
             PRAGMA foreign_keys = ON;\
             PRAGMA busy_timeout = 5000;\
             PRAGMA cache_size   = -8000;\
             PRAGMA mmap_size    = 268435456;\
             PRAGMA wal_autocheckpoint = 1000;\
             PRAGMA synchronous  = NORMAL;",
        )?;
        Ok(())
    });

    let pool = Pool::builder()
        .max_size(5)
        .connection_timeout(std::time::Duration::from_secs(10))
        .build(manager)
        .expect("failed to create SQLite connection pool");

    run_migrations(&pool);
    repair_schema_gaps(&pool);
    purge_garbage_edits(&pool);
    if crate::legacy_learning::legacy_learning_writes_allowed() {
        corrections::backfill_from_edit_events(&pool);
        let repaired_term_types = vocabulary::backfill_missing_term_types(&pool);
        let rebuilt_fts_rows = vocab_fts::backfill_from_vocabulary(&pool);
        if repaired_term_types > 0 || rebuilt_fts_rows > 0 {
            info!(
                "[vocab-repair] startup repaired term_types={} fts_rows={}",
                repaired_term_types, rebuilt_fts_rows,
            );
        }
    } else {
        info!("[legacy-learning] skipped startup legacy learning backfills — writes frozen");
    }
    pool
}

fn run_migrations(pool: &DbPool) {
    let conn = pool.get().expect("pool get failed during migration");

    // Check schema version
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap_or(0);

    if version < 1 {
        info!("running migration 001_initial");
        conn.execute_batch(MIGRATION_001)
            .expect("migration 001 failed");
        conn.execute_batch("PRAGMA user_version = 1")
            .expect("failed to set user_version");
    }

    if version < 2 {
        info!("running migration 002_vectors");
        conn.execute_batch(MIGRATION_002)
            .expect("migration 002 failed");
        conn.execute_batch("PRAGMA user_version = 2")
            .expect("failed to set user_version to 2");
    }

    if version < 3 {
        info!("running migration 003_output_language");
        conn.execute_batch(MIGRATION_003)
            .expect("migration 003 failed");
        conn.execute_batch("PRAGMA user_version = 3")
            .expect("failed to set user_version to 3");
    }

    if version < 4 {
        info!("running migration 004_api_keys");
        conn.execute_batch(MIGRATION_004)
            .expect("migration 004 failed");
        conn.execute_batch("PRAGMA user_version = 4")
            .expect("failed to set user_version to 4");
    }

    if version < 5 {
        info!("running migration 005_llm_provider");
        conn.execute_batch(MIGRATION_005)
            .expect("migration 005 failed");
        conn.execute_batch("PRAGMA user_version = 5")
            .expect("failed to set user_version to 5");
    }

    if version < 6 {
        info!("running migration 006_openai_oauth");
        conn.execute_batch(MIGRATION_006)
            .expect("migration 006 failed");
        conn.execute_batch("PRAGMA user_version = 6")
            .expect("failed to set user_version to 6");
    }

    if version < 7 {
        info!("running migration 007_pending_edits");
        conn.execute_batch(MIGRATION_007)
            .expect("migration 007 failed");
        conn.execute_batch("PRAGMA user_version = 7")
            .expect("failed to set user_version to 7");
    }

    if version < 8 {
        info!("running migration 008_recording_audio_id");
        conn.execute_batch(MIGRATION_008)
            .expect("migration 008 failed");
        conn.execute_batch("PRAGMA user_version = 8")
            .expect("failed to set user_version to 8");
    }

    if version < 9 {
        info!("running migration 009_word_corrections");
        conn.execute_batch(MIGRATION_009)
            .expect("migration 009 failed");
        conn.execute_batch("PRAGMA user_version = 9")
            .expect("failed to set user_version to 9");
    }

    if version < 10 {
        info!("running migration 010_groq_api_key");
        conn.execute_batch(MIGRATION_010)
            .expect("migration 010 failed");
        conn.execute_batch("PRAGMA user_version = 10")
            .expect("failed to set user_version to 10");
    }

    if version < 11 {
        info!(
            "running migration 011_embed_dims_256 — clearing 768-dim vectors for 256-dim rebuild"
        );
        conn.execute_batch(MIGRATION_011)
            .expect("migration 011 failed");
        conn.execute_batch("PRAGMA user_version = 11")
            .expect("failed to set user_version to 11");
    }

    if version < 12 {
        info!("running migration 012_vocabulary_and_stt_replacements");
        conn.execute_batch(MIGRATION_012)
            .expect("migration 012 failed");
        conn.execute_batch("PRAGMA user_version = 12")
            .expect("failed to set user_version to 12");
    }

    if version < 13 {
        info!("running migration 013_pending_promotions_and_language");
        conn.execute_batch(MIGRATION_013)
            .expect("migration 013 failed");
        conn.execute_batch("PRAGMA user_version = 13")
            .expect("failed to set user_version to 13");
    }

    if version < 14 {
        info!("running migration 014_vocabulary_example_context");
        conn.execute_batch(MIGRATION_014)
            .expect("migration 014 failed");
        conn.execute_batch("PRAGMA user_version = 14")
            .expect("failed to set user_version to 14");
    }

    if version < 15 {
        info!("running migration 015_vocab_embeddings");
        conn.execute_batch(MIGRATION_015)
            .expect("migration 015 failed");
        conn.execute_batch("PRAGMA user_version = 15")
            .expect("failed to set user_version to 15");
    }

    if version < 16 {
        info!("running migration 016_vocab_term_type");
        conn.execute_batch(MIGRATION_016)
            .expect("migration 016 failed");
        conn.execute_batch("PRAGMA user_version = 16")
            .expect("failed to set user_version to 16");
    }

    if version < 17 {
        info!("running migration 017_centroid_decay_fts");
        conn.execute_batch(MIGRATION_017)
            .expect("migration 017 failed");
        conn.execute_batch("PRAGMA user_version = 17")
            .expect("failed to set user_version to 17");
    }

    if version < 18 {
        info!("running migration 018_vocab_meaning");
        conn.execute_batch(MIGRATION_018)
            .expect("migration 018 failed");
        conn.execute_batch("PRAGMA user_version = 18")
            .expect("failed to set user_version to 18");
    }

    if version < 19 {
        info!("running migration 019_background_learning_trust");
        conn.execute_batch(MIGRATION_019)
            .expect("migration 019 failed");
        conn.execute_batch("PRAGMA user_version = 19")
            .expect("failed to set user_version to 19");
    }

    if version < 20 {
        info!("running migration 020_record_hotkey");
        conn.execute_batch(MIGRATION_020)
            .expect("migration 020 failed");
        conn.execute_batch("PRAGMA user_version = 20")
            .expect("failed to set user_version to 20");
    }

    if version < 21 {
        info!("running migration 021_learning_enabled");
        conn.execute_batch(MIGRATION_021)
            .expect("migration 021 failed");
        conn.execute_batch("PRAGMA user_version = 21")
            .expect("failed to set user_version to 21");
    }

    if version < 22 {
        info!("running migration 022_fix_recording_seconds");
        conn.execute_batch(MIGRATION_022)
            .expect("migration 022 failed");
        conn.execute_batch("PRAGMA user_version = 22")
            .expect("failed to set user_version to 22");
    }

    if version < 23 {
        info!("running migration 023_enriched_transcript");
        conn.execute_batch(MIGRATION_023)
            .expect("migration 023 failed");
        conn.execute_batch("PRAGMA user_version = 23")
            .expect("failed to set user_version to 23");
    }

    if version < 24 {
        info!("running migration 024_prompt_templates");
        conn.execute_batch(MIGRATION_024)
            .expect("migration 024 failed");
        conn.execute_batch("PRAGMA user_version = 24")
            .expect("failed to set user_version to 24");
    }

    if version < 25 {
        info!("running migration 025_pending_edits_notified");
        conn.execute_batch(MIGRATION_025)
            .expect("migration 025 failed");
        conn.execute_batch("PRAGMA user_version = 25")
            .expect("failed to set user_version to 25");
    }

    if version < 26 {
        info!("running migration 026_cerebras_api_key");
        conn.execute_batch(MIGRATION_026)
            .expect("migration 026 failed");
        conn.execute_batch("PRAGMA user_version = 26")
            .expect("failed to set user_version to 26");
    }

    if version < 27 {
        info!("running migration 027_tier2_model_metadata");
        conn.execute_batch(MIGRATION_027)
            .expect("migration 027 failed");
        conn.execute_batch("PRAGMA user_version = 27")
            .expect("failed to set user_version to 27");
    }

    if version < 28 {
        info!("running migration 028_tier2_policy_learning");
        conn.execute_batch(MIGRATION_028)
            .expect("migration 028 failed");
        conn.execute_batch("PRAGMA user_version = 28")
            .expect("failed to set user_version to 28");
    }

    if version < 29 {
        info!("running migration 029_tier2_edit_policy");
        conn.execute_batch(MIGRATION_029)
            .expect("migration 029 failed");
        conn.execute_batch("PRAGMA user_version = 29")
            .expect("failed to set user_version to 29");
    }

    if version < 30 {
        info!("running migration 030_stt_provider");
        conn.execute_batch(MIGRATION_030)
            .expect("migration 030 failed");
        conn.execute_batch("PRAGMA user_version = 30")
            .expect("failed to set user_version to 30");
    }

    if version < 31 {
        info!("running migration 031_alias_safety_judgments");
        conn.execute_batch(MIGRATION_031)
            .expect("migration 031 failed");
        conn.execute_batch("PRAGMA user_version = 31")
            .expect("failed to set user_version to 31");
    }

    if version < 32 {
        info!("running migration 032_enterprise_server_url");
        conn.execute_batch(MIGRATION_032)
            .expect("migration 032 failed");
        conn.execute_batch("PRAGMA user_version = 32")
            .expect("failed to set user_version to 32");
    }

    if version < 33 {
        info!("running migration 033_email_memory");
        conn.execute_batch(MIGRATION_033)
            .expect("migration 033 failed");
        conn.execute_batch("PRAGMA user_version = 33")
            .expect("failed to set user_version to 33");
    }

    if version < 34 {
        info!("running migration 034_company_vocab");
        conn.execute_batch(MIGRATION_034)
            .expect("migration 034 failed");
        conn.execute_batch("PRAGMA user_version = 34")
            .expect("failed to set user_version to 34");
    }

    if version < 35 {
        info!("running migration 035_server_runtime_probe");
        conn.execute_batch(MIGRATION_035)
            .expect("migration 035 failed");
        conn.execute_batch("PRAGMA user_version = 35")
            .expect("failed to set user_version to 35");
    }

    if version < 36 {
        info!("running migration 036_server_audio_runtime_probe");
        conn.execute_batch(MIGRATION_036)
            .expect("migration 036 failed");
        conn.execute_batch("PRAGMA user_version = 36")
            .expect("failed to set user_version to 36");
    }

    if version < 37 {
        info!("running migration 037_server_migration_state");
        conn.execute_batch(MIGRATION_037)
            .expect("migration 037 failed");
        conn.execute_batch("PRAGMA user_version = 37")
            .expect("failed to set user_version to 37");
    }

    if version < 38 {
        info!("running migration 038_server_settings_state");
        conn.execute_batch(MIGRATION_038)
            .expect("migration 038 failed");
        conn.execute_batch("PRAGMA user_version = 38")
            .expect("failed to set user_version to 38");
    }

    if version < 39 {
        info!("running migration 039_enable_server_runtime_for_signed_in");
        conn.execute_batch(MIGRATION_039)
            .expect("migration 039 failed");
        conn.execute_batch("PRAGMA user_version = 39")
            .expect("failed to set user_version to 39");
    }

    if version < 40 {
        info!("running migration 040_active_org_id");
        conn.execute_batch(MIGRATION_040)
            .expect("migration 040 failed");
        conn.execute_batch("PRAGMA user_version = 40")
            .expect("failed to set user_version to 40");
    }

    if version < 41 {
        info!("running migration 041_telemetry");
        conn.execute_batch(MIGRATION_041)
            .expect("migration 041 failed");
        conn.execute_batch("PRAGMA user_version = 41")
            .expect("failed to set user_version to 41");
    }

    if version < 42 {
        info!("running migration 042_telemetry_stt");
        conn.execute_batch(MIGRATION_042)
            .expect("migration 042 failed");
        conn.execute_batch("PRAGMA user_version = 42")
            .expect("failed to set user_version to 42");
    }

    if version < 43 {
        info!("running migration 043_sarvam_api_key");
        conn.execute_batch(MIGRATION_043)
            .expect("migration 043 failed");
        conn.execute_batch("PRAGMA user_version = 43")
            .expect("failed to set user_version to 43");
    }

    if version < 44 {
        info!("running migration 044_normalize_deepseek_polish_model");
        conn.execute_batch(MIGRATION_044)
            .expect("migration 044 failed");
        conn.execute_batch("PRAGMA user_version = 44")
            .expect("failed to set user_version to 44");
    }

    if version < 45 {
        info!("running migration 045_deepinfra_api_key");
        conn.execute_batch(MIGRATION_045)
            .expect("migration 045 failed");
        conn.execute_batch("PRAGMA user_version = 45")
            .expect("failed to set user_version to 45");
    }

    if version < 46 {
        info!("running migration 046_force_server_runtime");
        conn.execute_batch(MIGRATION_046)
            .expect("migration 046 failed");
        conn.execute_batch("PRAGMA user_version = 46")
            .expect("failed to set user_version to 46");
    }

    if version < 47 {
        info!("running migration 047_default_gpt_oss_20b");
        conn.execute_batch(MIGRATION_047)
            .expect("migration 047 failed");
        conn.execute_batch("PRAGMA user_version = 47")
            .expect("failed to set user_version to 47");
    }

    if version < 48 {
        info!("running migration 048_default_cerebras_gpt_oss_120b");
        conn.execute_batch(MIGRATION_048)
            .expect("migration 048 failed");
        conn.execute_batch("PRAGMA user_version = 48")
            .expect("failed to set user_version to 48");
    }

    if version < 49 {
        info!("running migration 049_lock_cerebras_polish_defaults");
        conn.execute_batch(MIGRATION_049)
            .expect("migration 049 failed");
        conn.execute_batch("PRAGMA user_version = 49")
            .expect("failed to set user_version to 49");
    }

    if version < 50 {
        info!("running migration 050_local_profile_summary");
        conn.execute_batch(MIGRATION_050)
            .expect("migration 050 failed");
        conn.execute_batch("PRAGMA user_version = 50")
            .expect("failed to set user_version to 50");
    }

    if version < 51 {
        info!("running migration 051_observability_outbox");
        conn.execute_batch(MIGRATION_051)
            .expect("migration 051 failed");
        conn.execute_batch("PRAGMA user_version = 51")
            .expect("failed to set user_version to 51");
    }

    if version < 52 {
        info!("running migration 052_voice_runs");
        conn.execute_batch(MIGRATION_052)
            .expect("migration 052 failed");
        conn.execute_batch("PRAGMA user_version = 52")
            .expect("failed to set user_version to 52");
    }

    if version < 53 {
        info!("running migration 053_retire_swift_local_stt");
        conn.execute_batch(MIGRATION_053)
            .expect("migration 053 failed");
        conn.execute_batch("PRAGMA user_version = 53")
            .expect("failed to set user_version to 53");
    }

    if version < 54 {
        info!("running migration 054_recording_trace_json");
        conn.execute_batch(MIGRATION_054)
            .expect("migration 054 failed");
        conn.execute_batch("PRAGMA user_version = 54")
            .expect("failed to set user_version to 54");
    }

    if version < 55 {
        info!("running migration 055_drop_prompt_templates");
        conn.execute_batch(MIGRATION_055)
            .expect("migration 055 failed");
        conn.execute_batch("PRAGMA user_version = 55")
            .expect("failed to set user_version to 55");
    }

    if version < 56 {
        info!("running migration 056_site_visits");
        conn.execute_batch(MIGRATION_056)
            .expect("migration 056 failed");
        conn.execute_batch("PRAGMA user_version = 56")
            .expect("failed to set user_version to 56");
    }
}

/// Idempotent repairs for partial migration states (e.g. user_version bumped without ALTER).
fn repair_schema_gaps(pool: &DbPool) {
    let conn = match pool.get() {
        Ok(c) => c,
        Err(e) => {
            warn!("[schema-repair] pool get failed: {e}");
            return;
        }
    };

    fn has_column(conn: &Connection, table: &str, column: &str) -> bool {
        let sql = format!("SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name = ?1");
        conn.query_row(&sql, [column], |row| row.get::<_, i64>(0))
            .map(|n| n > 0)
            .unwrap_or(false)
    }

    fn add_column_if_missing(conn: &Connection, table: &str, column: &str, definition: &str) {
        if has_column(conn, table, column) {
            return;
        }
        warn!("[schema-repair] adding missing {table}.{column}");
        let sql = format!("ALTER TABLE {table} ADD COLUMN {definition};");
        if let Err(e) = conn.execute_batch(&sql) {
            warn!("[schema-repair] {table}.{column} add failed: {e}");
        }
    }

    add_column_if_missing(&conn, "local_user", "active_org_id", "active_org_id TEXT");

    // Older dev builds occasionally advanced user_version while some ALTER
    // statements were missing. Keep this startup repair idempotent so prefs
    // reads do not fail forever on partially migrated local databases.
    add_column_if_missing(
        &conn,
        "preferences",
        "output_language",
        "output_language TEXT NOT NULL DEFAULT 'hinglish'",
    );
    add_column_if_missing(
        &conn,
        "preferences",
        "gateway_api_key",
        "gateway_api_key TEXT",
    );
    add_column_if_missing(
        &conn,
        "preferences",
        "deepgram_api_key",
        "deepgram_api_key TEXT",
    );
    add_column_if_missing(
        &conn,
        "preferences",
        "gemini_api_key",
        "gemini_api_key TEXT",
    );
    add_column_if_missing(
        &conn,
        "preferences",
        "llm_provider",
        "llm_provider TEXT NOT NULL DEFAULT 'gateway'",
    );
    add_column_if_missing(&conn, "preferences", "groq_api_key", "groq_api_key TEXT");
    add_column_if_missing(
        &conn,
        "preferences",
        "record_hotkey",
        "record_hotkey TEXT NOT NULL DEFAULT 'caps_lock'",
    );
    add_column_if_missing(
        &conn,
        "preferences",
        "learning_enabled",
        "learning_enabled INTEGER NOT NULL DEFAULT 1",
    );
    add_column_if_missing(
        &conn,
        "preferences",
        "cerebras_api_key",
        "cerebras_api_key TEXT",
    );
    add_column_if_missing(
        &conn,
        "preferences",
        "stt_provider",
        "stt_provider TEXT NOT NULL DEFAULT 'deepgram'",
    );
    add_column_if_missing(
        &conn,
        "preferences",
        "server_runtime_enabled",
        "server_runtime_enabled INTEGER NOT NULL DEFAULT 0",
    );
    add_column_if_missing(
        &conn,
        "preferences",
        "server_audio_runtime_enabled",
        "server_audio_runtime_enabled INTEGER NOT NULL DEFAULT 0",
    );
    add_column_if_missing(
        &conn,
        "preferences",
        "sarvam_api_key",
        "sarvam_api_key TEXT",
    );
    add_column_if_missing(
        &conn,
        "preferences",
        "deepinfra_api_key",
        "deepinfra_api_key TEXT",
    );
    add_column_if_missing(&conn, "recordings", "trace_json", "trace_json TEXT");
}

/// Return the default database path. Delegates to `paths::default_db_path()`
/// for cross-platform resolution. Kept here for backwards compatibility with
/// existing callers that import `store::default_db_path`.
pub fn default_db_path() -> PathBuf {
    crate::paths::default_db_path()
}

/// Ensure the single default local user exists.
/// Returns the user_id (UUID string).
pub fn ensure_default_user(pool: &DbPool) -> String {
    let conn = pool.get().expect("pool get");

    // Check if any user exists
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM local_user", [], |r| r.get(0))
        .unwrap_or(0);

    if count > 0 {
        // Return existing user id
        return conn
            .query_row("SELECT id FROM local_user LIMIT 1", [], |r| r.get(0))
            .expect("failed to read user id");
    }

    // Create default user
    let id = uuid::Uuid::new_v4().to_string();
    let now_ms = now_ms();
    conn.execute(
        "INSERT INTO local_user (id, email, license_tier, created_at)
         VALUES (?1, ?2, 'free', ?3)",
        params![id, "local@voicepolish.app", now_ms],
    )
    .expect("failed to create default user");

    // Create default preferences
    conn.execute(
        "INSERT INTO preferences (user_id, selected_model, tone_preset, language,
         auto_paste, edit_capture, polish_text_hotkey, record_hotkey, server_runtime_enabled, updated_at)
         VALUES (?1, 'cerebras-gpt-oss', 'neutral', 'auto', 1, 1, 'cmd+shift+p', 'caps_lock', 0, ?2)",
        params![id, now_ms],
    )
    .expect("failed to create default preferences");

    info!("created default local user: {id}");
    id
}

pub fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// Remove edit_events (and their linked preference_vectors) where user_kept
/// has no meaningful word overlap with ai_output — i.e. the watcher captured
/// a UI placeholder (e.g. Slack's "Type / for commands") instead of the real edit.
/// Runs once at startup so stale garbage never poisons future RAG retrievals.
fn purge_garbage_edits(pool: &DbPool) {
    let conn = match pool.get() {
        Ok(c) => c,
        Err(e) => {
            warn!("[purge] pool error: {e}");
            return;
        }
    };

    // Load all edit_events for inspection
    let rows: Vec<(String, String, String)> = {
        let mut stmt = match conn.prepare("SELECT id, ai_output, user_kept FROM edit_events") {
            Ok(s) => s,
            Err(_) => return,
        };
        stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .ok()
        .map(|it| it.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    };

    let mut deleted = 0usize;
    for (id, ai_output, user_kept) in &rows {
        if !has_word_overlap(user_kept, ai_output) {
            // Delete from preference_vectors first (JOIN dependency)
            let _ = conn.execute(
                "DELETE FROM preference_vectors WHERE edit_event_id = ?1",
                params![id],
            );
            if let Ok(n) = conn.execute("DELETE FROM edit_events WHERE id = ?1", params![id]) {
                if n > 0 {
                    deleted += 1;
                }
            }
        }
    }

    if deleted > 0 {
        info!("[purge] removed {deleted} garbage edit_event(s) with no word overlap");
    }
}

/// True if any word >3 chars from `a` appears (case-insensitive) in `b`.
fn has_word_overlap(a: &str, b: &str) -> bool {
    let b_words: std::collections::HashSet<String> = b
        .split_whitespace()
        .filter(|w| w.chars().count() > 3)
        .map(|w| w.to_lowercase())
        .collect();
    if b_words.is_empty() {
        return !a.trim().is_empty();
    }
    a.split_whitespace()
        .any(|w| b_words.contains(&w.to_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use r2d2_sqlite::SqliteConnectionManager;

    #[test]
    fn latest_migrations_create_tier2_and_alias_safety_tables() {
        let manager = SqliteConnectionManager::memory();
        let pool = Pool::builder().max_size(1).build(manager).unwrap();
        run_migrations(&pool);

        let conn = pool.get().unwrap();
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 56);

        for table in [
            "tier2_policy_weights",
            "tier2_decision_events",
            "tier2_edit_policy_rules",
            "alias_safety_judgments",
            "email_memories",
            "company_bucket_state",
            "company_vocabulary",
            "company_stt_replacements",
        ] {
            let exists: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    params![table],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(exists, 1, "{table} should exist after latest migrations");
        }

        for column in [
            "raw_transcript",
            "local_corrected_transcript",
            "polished_output",
        ] {
            let exists: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('recordings') WHERE name = ?1",
                    params![column],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(exists, 1, "recordings.{column} should exist");
        }
    }
}
