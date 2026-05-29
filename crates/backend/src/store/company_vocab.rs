//! Local mirror for enterprise company vocabulary buckets.
//!
//! The mirror is read from the dictation hot path. Network sync and summary
//! upload happen through background/manual endpoints only.

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use super::{
    DbPool, now_ms,
    stt_replacements::{ExportTier, ReviewStatus, SttReplacement},
    vocabulary::VocabTerm,
};
use crate::llm::phonetics;

#[derive(Debug, Clone, Serialize)]
pub struct CompanyBucketStatus {
    pub enabled: bool,
    pub org_id: Option<String>,
    pub version: i64,
    pub bucket_hash: Option<String>,
    pub last_checked_at: Option<i64>,
    pub last_synced_at: Option<i64>,
    pub last_error: Option<String>,
    pub term_count: i64,
    pub alias_count: i64,
    pub upload_last_uploaded_at: Option<i64>,
    pub upload_last_error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CompanyBucketResponse {
    pub org_id: String,
    pub version: i64,
    pub bucket_hash: Option<String>,
    pub manifest: CompanyBucketManifest,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CompanyBucketManifest {
    #[serde(default)]
    pub terms: Vec<CompanyBucketTerm>,
    #[serde(default)]
    pub aliases: Vec<CompanyBucketAlias>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CompanyBucketTerm {
    pub term: String,
    #[serde(default)]
    pub term_norm: Option<String>,
    #[serde(default)]
    pub term_type: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default = "default_weight")]
    pub weight: f64,
    #[serde(default)]
    pub priority: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CompanyBucketAlias {
    pub transcript_form: String,
    #[serde(default)]
    pub transcript_norm: Option<String>,
    pub correct_form: String,
    #[serde(default)]
    pub correct_norm: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default = "default_weight")]
    pub weight: f64,
    #[serde(default)]
    pub safety_status: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct UserVocabUpload {
    pub device_id: String,
    pub terms: Vec<UserVocabTermSummary>,
    pub aliases: Vec<UserVocabAliasSummary>,
    pub company_bucket_version: i64,
    pub company_vocab_synced_at: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct UserVocabTermSummary {
    pub term: String,
    pub term_norm: String,
    pub term_type: String,
    pub source: String,
    pub weight: f64,
    pub use_count: i64,
    pub positive_count: i64,
    pub negative_count: i64,
    pub safety_status: String,
    pub first_seen_at: Option<i64>,
    pub last_seen_at: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct UserVocabAliasSummary {
    pub transcript_form: String,
    pub transcript_norm: String,
    pub correct_form: String,
    pub correct_norm: String,
    pub weight: f64,
    pub use_count: i64,
    pub positive_count: i64,
    pub negative_count: i64,
    pub safety_status: String,
    pub review_status: String,
    pub first_seen_at: Option<i64>,
    pub last_seen_at: Option<i64>,
}

fn default_weight() -> f64 {
    1.0
}

pub fn normalize(raw: &str) -> String {
    raw.trim()
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c
            } else if c.is_whitespace() || matches!(c, '-' | '_' | '.') {
                ' '
            } else {
                '\0'
            }
        })
        .filter(|c| *c != '\0')
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn status(pool: &DbPool, user_id: &str) -> CompanyBucketStatus {
    let conn = match pool.get() {
        Ok(c) => c,
        Err(e) => {
            return CompanyBucketStatus {
                enabled: false,
                org_id: None,
                version: 0,
                bucket_hash: None,
                last_checked_at: None,
                last_synced_at: None,
                last_error: Some(format!("pool error: {e}")),
                term_count: 0,
                alias_count: 0,
                upload_last_uploaded_at: None,
                upload_last_error: None,
            };
        }
    };
    let state: Option<(
        Option<String>,
        i64,
        Option<String>,
        Option<i64>,
        Option<i64>,
        Option<String>,
    )> = conn
        .query_row(
            "SELECT org_id, version, bucket_hash, last_checked_at, last_synced_at, last_error
               FROM company_bucket_state WHERE user_id = ?1",
            params![user_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .optional()
        .unwrap_or(None);
    let term_count = conn
        .query_row(
            "SELECT COUNT(*) FROM company_vocabulary WHERE user_id = ?1 AND status = 'approved'",
            params![user_id],
            |row| row.get(0),
        )
        .unwrap_or(0);
    let alias_count = conn
        .query_row(
            "SELECT COUNT(*) FROM company_stt_replacements WHERE user_id = ?1 AND status = 'approved'",
            params![user_id],
            |row| row.get(0),
        )
        .unwrap_or(0);
    let upload_state: Option<(Option<i64>, Option<String>)> = conn
        .query_row(
            "SELECT last_uploaded_at, last_error FROM company_vocab_upload_state WHERE user_id = ?1",
            params![user_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .unwrap_or(None);
    let (org_id, version, bucket_hash, last_checked_at, last_synced_at, last_error) =
        state.unwrap_or((None, 0, None, None, None, None));
    let (upload_last_uploaded_at, upload_last_error) = upload_state.unwrap_or((None, None));
    CompanyBucketStatus {
        enabled: org_id.is_some() || version > 0,
        org_id,
        version,
        bucket_hash,
        last_checked_at,
        last_synced_at,
        last_error,
        term_count,
        alias_count,
        upload_last_uploaded_at,
        upload_last_error,
    }
}

pub fn should_check(pool: &DbPool, user_id: &str, force: bool) -> bool {
    if force {
        return true;
    }
    let status = status(pool, user_id);
    let Some(last_checked) = status.last_checked_at else {
        return true;
    };
    now_ms().saturating_sub(last_checked) >= 24 * 60 * 60 * 1000
}

pub fn mark_checked(
    pool: &DbPool,
    user_id: &str,
    org_id: Option<&str>,
    version: i64,
    bucket_hash: Option<&str>,
    error: Option<&str>,
) {
    if let Ok(conn) = pool.get() {
        let _ = conn.execute(
            "INSERT INTO company_bucket_state
                (user_id, org_id, version, bucket_hash, last_checked_at, last_error)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(user_id) DO UPDATE SET
                org_id = COALESCE(excluded.org_id, company_bucket_state.org_id),
                version = excluded.version,
                bucket_hash = COALESCE(excluded.bucket_hash, company_bucket_state.bucket_hash),
                last_checked_at = excluded.last_checked_at,
                last_error = excluded.last_error",
            params![user_id, org_id, version, bucket_hash, now_ms(), error],
        );
    }
}

pub fn replace_bucket(
    pool: &DbPool,
    user_id: &str,
    bucket: &CompanyBucketResponse,
) -> rusqlite::Result<()> {
    let mut conn = pool.get().map_err(|_| rusqlite::Error::InvalidQuery)?;
    let tx = conn.transaction()?;
    let now = now_ms();
    tx.execute(
        "DELETE FROM company_vocabulary WHERE user_id = ?1",
        params![user_id],
    )?;
    tx.execute(
        "DELETE FROM company_stt_replacements WHERE user_id = ?1",
        params![user_id],
    )?;
    for term in &bucket.manifest.terms {
        let term_text = term.term.trim();
        if term_text.is_empty() {
            continue;
        }
        let norm = term
            .term_norm
            .clone()
            .unwrap_or_else(|| normalize(term_text));
        tx.execute(
            "INSERT OR REPLACE INTO company_vocabulary
                (user_id, org_id, term, term_norm, term_type, language, weight, priority, status, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'approved', ?9)",
            params![
                user_id,
                bucket.org_id,
                term_text,
                norm,
                term.term_type.as_deref().unwrap_or("other"),
                term.language.as_deref().unwrap_or("hinglish"),
                term.weight,
                term.priority,
                now
            ],
        )?;
    }
    for alias in &bucket.manifest.aliases {
        let source = alias.transcript_form.trim();
        let correct = alias.correct_form.trim();
        if source.is_empty() || correct.is_empty() {
            continue;
        }
        let source_norm = alias
            .transcript_norm
            .clone()
            .unwrap_or_else(|| normalize(source));
        let correct_norm = alias
            .correct_norm
            .clone()
            .unwrap_or_else(|| normalize(correct));
        if alias.safety_status.as_deref() == Some("common_block") {
            tx.execute(
                "INSERT OR REPLACE INTO company_vocab_tombstones
                    (user_id, org_id, entity_kind, entity_norm, bucket_version, updated_at)
                 VALUES (?1, ?2, 'alias', ?3, ?4, ?5)",
                params![user_id, bucket.org_id, source_norm, bucket.version, now],
            )?;
            continue;
        }
        tx.execute(
            "INSERT OR REPLACE INTO company_stt_replacements
                (user_id, org_id, transcript_form, transcript_norm, correct_form, correct_norm,
                 language, weight, status, safety_status, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'approved', ?9, ?10)",
            params![
                user_id,
                bucket.org_id,
                source,
                source_norm,
                correct,
                correct_norm,
                alias.language.as_deref().unwrap_or("hinglish"),
                alias.weight,
                alias.safety_status.as_deref().unwrap_or("safe_jargon"),
                now
            ],
        )?;
    }
    tx.execute(
        "INSERT INTO company_bucket_state
            (user_id, org_id, version, bucket_hash, last_checked_at, last_synced_at, last_error)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5, NULL)
         ON CONFLICT(user_id) DO UPDATE SET
            org_id = excluded.org_id,
            version = excluded.version,
            bucket_hash = excluded.bucket_hash,
            last_checked_at = excluded.last_checked_at,
            last_synced_at = excluded.last_synced_at,
            last_error = NULL",
        params![
            user_id,
            bucket.org_id,
            bucket.version,
            bucket.bucket_hash.as_deref(),
            now
        ],
    )?;
    tx.commit()
}

pub fn load_terms(pool: &DbPool, user_id: &str, limit: usize) -> Vec<VocabTerm> {
    let conn = match pool.get() {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let mut stmt = match conn.prepare(
        "SELECT term, weight, priority, updated_at, term_type
           FROM company_vocabulary
          WHERE user_id = ?1 AND status = 'approved'
          ORDER BY priority DESC, weight DESC, term ASC
          LIMIT ?2",
    ) {
        Ok(stmt) => stmt,
        Err(_) => return Vec::new(),
    };
    let rows = match stmt.query_map(params![user_id, limit as i64], |row| {
        let priority: i64 = row.get(2)?;
        Ok(VocabTerm {
            term: row.get(0)?,
            weight: row.get::<_, f64>(1)? + (priority as f64 * 0.1),
            use_count: 0,
            last_used: row.get(3)?,
            source: "company".to_string(),
            example_context: None,
            term_type: row.get(4)?,
            meaning: None,
        })
    }) {
        Ok(rows) => rows,
        Err(_) => return Vec::new(),
    };
    rows.filter_map(|r| r.ok()).collect()
}

pub fn load_aliases(pool: &DbPool, user_id: &str) -> Vec<SttReplacement> {
    let conn = match pool.get() {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let mut stmt = match conn.prepare(
        "SELECT transcript_form, correct_form, weight, updated_at, language, safety_status
           FROM company_stt_replacements
          WHERE user_id = ?1 AND status = 'approved' AND safety_status <> 'common_block'",
    ) {
        Ok(stmt) => stmt,
        Err(_) => return Vec::new(),
    };
    let rows = match stmt.query_map(params![user_id], |row| {
        let transcript: String = row.get(0)?;
        Ok(SttReplacement {
            phonetic_key: phonetics::phonetic_key(&transcript),
            transcript_form: transcript,
            correct_form: row.get(1)?,
            weight: row.get(2)?,
            use_count: 100,
            last_used: row.get(3)?,
            language: row.get(4)?,
            export_tier: ExportTier::ExportReplaceReady,
            contradiction_count: 0,
            review_status: ReviewStatus::Approved,
            review_reason: Some("company_bucket".to_string()),
            last_reviewed_at: None,
        })
    }) {
        Ok(rows) => rows,
        Err(_) => return Vec::new(),
    };
    rows.filter_map(|r| r.ok()).collect()
}

pub fn starred_or_priority_terms(pool: &DbPool, user_id: &str, limit: usize) -> Vec<String> {
    let mut terms = super::vocabulary::starred_term_strings(pool, user_id);
    let company = load_terms(pool, user_id, limit);
    for term in company {
        if !terms.iter().any(|t| t.eq_ignore_ascii_case(&term.term)) {
            terms.push(term.term);
        }
        if terms.len() >= limit {
            break;
        }
    }
    terms
}

pub fn build_user_summary(pool: &DbPool, user_id: &str, device_id: String) -> UserVocabUpload {
    let status = status(pool, user_id);
    let terms = super::vocabulary::top_terms(pool, user_id, 500)
        .into_iter()
        .map(|t| UserVocabTermSummary {
            term_norm: normalize(&t.term),
            term_type: t.term_type.unwrap_or_else(|| "other".to_string()),
            source: t.source,
            weight: t.weight,
            use_count: t.use_count,
            positive_count: t.use_count.max(0),
            negative_count: 0,
            safety_status: "local".to_string(),
            first_seen_at: None,
            last_seen_at: Some(t.last_used),
            term: t.term,
        })
        .collect();
    let aliases = super::stt_replacements::load_all(pool, user_id)
        .into_iter()
        .filter(|r| {
            r.review_status != ReviewStatus::Blocked && r.export_tier != ExportTier::Blocked
        })
        .map(|r| UserVocabAliasSummary {
            transcript_norm: normalize(&r.transcript_form),
            correct_norm: normalize(&r.correct_form),
            weight: r.weight,
            use_count: r.use_count,
            positive_count: r.use_count.max(0),
            negative_count: r.contradiction_count.max(0),
            safety_status: r
                .review_reason
                .clone()
                .unwrap_or_else(|| "local".to_string()),
            review_status: r.review_status.as_str().to_string(),
            first_seen_at: None,
            last_seen_at: Some(r.last_used),
            transcript_form: r.transcript_form,
            correct_form: r.correct_form,
        })
        .collect();
    UserVocabUpload {
        device_id,
        terms,
        aliases,
        company_bucket_version: status.version,
        company_vocab_synced_at: status.last_synced_at,
    }
}

pub fn mark_upload_result(pool: &DbPool, user_id: &str, error: Option<&str>) {
    if let Ok(conn) = pool.get() {
        let _ = conn.execute(
            "INSERT INTO company_vocab_upload_state (user_id, last_uploaded_at, last_error)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(user_id) DO UPDATE SET
                last_uploaded_at = excluded.last_uploaded_at,
                last_error = excluded.last_error",
            params![user_id, now_ms(), error],
        );
    }
}

pub fn should_upload_summary(pool: &DbPool, user_id: &str, force: bool) -> bool {
    if force {
        return true;
    }
    let last_uploaded = status(pool, user_id).upload_last_uploaded_at;
    match last_uploaded {
        Some(ms) => now_ms().saturating_sub(ms) >= 24 * 60 * 60 * 1000,
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use r2d2_sqlite::SqliteConnectionManager;

    fn mem_pool() -> DbPool {
        let mgr = SqliteConnectionManager::memory();
        let pool = r2d2::Pool::builder().max_size(1).build(mgr).unwrap();
        pool.get()
            .unwrap()
            .execute_batch(include_str!("migrations/034_company_vocab.sql"))
            .unwrap();
        pool
    }

    #[test]
    fn replace_bucket_loads_company_terms_and_aliases() {
        let pool = mem_pool();
        let bucket = CompanyBucketResponse {
            org_id: "org-1".to_string(),
            version: 3,
            bucket_hash: Some("hash".to_string()),
            manifest: CompanyBucketManifest {
                terms: vec![CompanyBucketTerm {
                    term: "Macobs".to_string(),
                    term_norm: None,
                    term_type: Some("brand".to_string()),
                    language: Some("hinglish".to_string()),
                    weight: 2.0,
                    priority: 9,
                }],
                aliases: vec![
                    CompanyBucketAlias {
                        transcript_form: "mecobs".to_string(),
                        transcript_norm: None,
                        correct_form: "Macobs".to_string(),
                        correct_norm: None,
                        language: Some("hinglish".to_string()),
                        weight: 2.0,
                        safety_status: Some("safe_jargon".to_string()),
                    },
                    CompanyBucketAlias {
                        transcript_form: "kaisa".to_string(),
                        transcript_norm: None,
                        correct_form: "Macobs".to_string(),
                        correct_norm: None,
                        language: Some("hinglish".to_string()),
                        weight: 2.0,
                        safety_status: Some("common_block".to_string()),
                    },
                ],
            },
        };

        replace_bucket(&pool, "u1", &bucket).unwrap();

        let status = status(&pool, "u1");
        assert_eq!(status.version, 3);
        assert_eq!(status.term_count, 1);
        assert_eq!(status.alias_count, 1);

        let terms = load_terms(&pool, "u1", 10);
        assert_eq!(terms[0].term, "Macobs");
        assert_eq!(terms[0].source, "company");

        let aliases = load_aliases(&pool, "u1");
        assert_eq!(aliases.len(), 1);
        assert_eq!(aliases[0].transcript_form, "mecobs");
        assert_eq!(aliases[0].correct_form, "Macobs");
    }
}
