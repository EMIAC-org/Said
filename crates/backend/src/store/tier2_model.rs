use std::path::PathBuf;

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use super::{DbPool, now_ms};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Tier2ModelMetadata {
    pub user_id: String,
    pub artifact_path: String,
    pub vocab_index_path: String,
    pub trained_at: i64,
    pub alias_count: i64,
    pub vocab_count: i64,
    pub data_fingerprint: Option<String>,
    pub metrics_json: Option<String>,
    pub last_error: Option<String>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tier2ModelMetadataUpsert {
    pub artifact_path: String,
    pub vocab_index_path: String,
    pub trained_at: i64,
    pub alias_count: i64,
    pub vocab_count: i64,
    pub data_fingerprint: Option<String>,
    pub metrics_json: Option<String>,
    pub last_error: Option<String>,
}

pub fn artifact_dir(user_id: &str) -> PathBuf {
    crate::paths::data_dir().join("tier2").join(user_id)
}

pub fn default_artifact_path(user_id: &str) -> PathBuf {
    artifact_dir(user_id).join("correction_model.onnx")
}

pub fn default_vocab_index_path(user_id: &str) -> PathBuf {
    artifact_dir(user_id).join("vocab_index.json")
}

pub fn get(pool: &DbPool, user_id: &str) -> Option<Tier2ModelMetadata> {
    let conn = pool.get().ok()?;
    conn.query_row(
        "SELECT user_id, artifact_path, vocab_index_path, trained_at,
                alias_count, vocab_count, data_fingerprint, metrics_json,
                last_error, updated_at
           FROM tier2_model_metadata
          WHERE user_id = ?1",
        params![user_id],
        |row| {
            Ok(Tier2ModelMetadata {
                user_id: row.get(0)?,
                artifact_path: row.get(1)?,
                vocab_index_path: row.get(2)?,
                trained_at: row.get(3)?,
                alias_count: row.get(4)?,
                vocab_count: row.get(5)?,
                data_fingerprint: row.get(6)?,
                metrics_json: row.get(7)?,
                last_error: row.get(8)?,
                updated_at: row.get(9)?,
            })
        },
    )
    .optional()
    .ok()
    .flatten()
}

pub fn upsert(pool: &DbPool, user_id: &str, metadata: &Tier2ModelMetadataUpsert) -> bool {
    let Ok(conn) = pool.get() else {
        return false;
    };
    let updated_at = now_ms();
    conn.execute(
        "INSERT INTO tier2_model_metadata
             (user_id, artifact_path, vocab_index_path, trained_at,
              alias_count, vocab_count, data_fingerprint, metrics_json,
              last_error, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(user_id) DO UPDATE SET
             artifact_path = excluded.artifact_path,
             vocab_index_path = excluded.vocab_index_path,
             trained_at = excluded.trained_at,
             alias_count = excluded.alias_count,
             vocab_count = excluded.vocab_count,
             data_fingerprint = excluded.data_fingerprint,
             metrics_json = excluded.metrics_json,
             last_error = excluded.last_error,
             updated_at = excluded.updated_at",
        params![
            user_id,
            metadata.artifact_path,
            metadata.vocab_index_path,
            metadata.trained_at,
            metadata.alias_count,
            metadata.vocab_count,
            metadata.data_fingerprint.as_deref(),
            metadata.metrics_json.as_deref(),
            metadata.last_error.as_deref(),
            updated_at,
        ],
    )
    .map(|rows| rows > 0)
    .unwrap_or(false)
}

pub fn set_last_error(pool: &DbPool, user_id: &str, last_error: Option<&str>) -> bool {
    let Ok(conn) = pool.get() else {
        return false;
    };
    conn.execute(
        "UPDATE tier2_model_metadata
            SET last_error = ?2,
                updated_at = ?3
          WHERE user_id = ?1",
        params![user_id, last_error, now_ms()],
    )
    .map(|rows| rows > 0)
    .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use r2d2_sqlite::SqliteConnectionManager;

    use super::*;

    fn mem_pool() -> DbPool {
        let mgr = SqliteConnectionManager::memory();
        let pool = r2d2::Pool::builder().max_size(1).build(mgr).unwrap();
        let conn = pool.get().unwrap();
        conn.execute_batch(
            "CREATE TABLE local_user (id TEXT PRIMARY KEY);
             INSERT INTO local_user(id) VALUES ('u1');
             CREATE TABLE tier2_model_metadata (
                 user_id           TEXT PRIMARY KEY REFERENCES local_user(id) ON DELETE CASCADE,
                 artifact_path     TEXT NOT NULL,
                 vocab_index_path  TEXT NOT NULL,
                 trained_at        INTEGER NOT NULL,
                 alias_count       INTEGER NOT NULL DEFAULT 0,
                 vocab_count       INTEGER NOT NULL DEFAULT 0,
                 data_fingerprint  TEXT,
                 metrics_json      TEXT,
                 last_error        TEXT,
                 updated_at        INTEGER NOT NULL
             );",
        )
        .unwrap();
        pool
    }

    #[test]
    fn tier2_metadata_round_trip() {
        let pool = mem_pool();
        let metadata = Tier2ModelMetadataUpsert {
            artifact_path: std::env::temp_dir()
                .join("correction_model.onnx")
                .to_string_lossy()
                .into_owned(),
            vocab_index_path: std::env::temp_dir()
                .join("vocab_index.json")
                .to_string_lossy()
                .into_owned(),
            trained_at: 123,
            alias_count: 4,
            vocab_count: 7,
            data_fingerprint: Some("abc".to_string()),
            metrics_json: Some("{\"auc\":0.9}".to_string()),
            last_error: None,
        };

        assert!(upsert(&pool, "u1", &metadata));
        let loaded = get(&pool, "u1").expect("metadata should exist");
        assert_eq!(loaded.artifact_path, metadata.artifact_path);
        assert_eq!(loaded.vocab_index_path, metadata.vocab_index_path);
        assert_eq!(loaded.alias_count, 4);
        assert_eq!(loaded.vocab_count, 7);
        assert_eq!(loaded.data_fingerprint.as_deref(), Some("abc"));

        assert!(set_last_error(&pool, "u1", Some("bad artifact")));
        let loaded = get(&pool, "u1").expect("metadata should exist");
        assert_eq!(loaded.last_error.as_deref(), Some("bad artifact"));
    }
}
