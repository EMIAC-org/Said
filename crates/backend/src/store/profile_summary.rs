use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tracing::{info, warn};

use super::{
    DbPool, now_ms,
    stt_replacements::{self, ReviewStatus},
    vocabulary,
};

const MAX_PROFILE_CHARS: usize = 4_000;
const MAX_TERMS: usize = 50;
const MAX_ALIASES: usize = 80;
const MAX_RECENT_TEXTS: usize = 8;

#[derive(Debug, Clone)]
pub struct CachedProfileSummary {
    pub profile_markdown: String,
    pub version: i64,
    pub source_hash: String,
    pub source_counts_json: String,
    pub updated_at: i64,
}

#[derive(Debug, Serialize)]
struct SourceSnapshot {
    terms: Vec<TermSnapshot>,
    aliases: Vec<AliasSnapshot>,
    recent_outputs: Vec<String>,
}

#[derive(Debug, Serialize)]
struct TermSnapshot {
    term: String,
    term_type: Option<String>,
    meaning: Option<String>,
    context: Option<String>,
}

#[derive(Debug, Serialize)]
struct AliasSnapshot {
    heard: String,
    correct: String,
    status: String,
}

pub fn get_cached(pool: &DbPool, user_id: &str) -> Option<CachedProfileSummary> {
    let conn = pool.get().ok()?;
    conn.query_row(
        "SELECT profile_markdown, version, source_hash, source_counts_json, updated_at
           FROM local_profile_summary
          WHERE user_id = ?1",
        params![user_id],
        |row| {
            Ok(CachedProfileSummary {
                profile_markdown: row.get(0)?,
                version: row.get(1)?,
                source_hash: row.get(2)?,
                source_counts_json: row.get(3)?,
                updated_at: row.get(4)?,
            })
        },
    )
    .optional()
    .ok()
    .flatten()
}

pub fn ensure_current(pool: &DbPool, user_id: &str) -> Option<CachedProfileSummary> {
    let snapshot = build_snapshot(pool, user_id);
    if snapshot.terms.is_empty()
        && snapshot.aliases.is_empty()
        && snapshot.recent_outputs.is_empty()
    {
        return None;
    }
    let source_json = serde_json::to_string(&snapshot).ok()?;
    let source_hash = hash(&source_json);
    if let Some(existing) = get_cached(pool, user_id) {
        if existing.source_hash == source_hash && !existing.profile_markdown.trim().is_empty() {
            info!(
                "[profile-summary] cache hit version={} chars={} terms={} aliases={} recent={}",
                existing.version,
                existing.profile_markdown.chars().count(),
                snapshot.terms.len(),
                snapshot.aliases.len(),
                snapshot.recent_outputs.len(),
            );
            return Some(existing);
        }
    }
    rebuild_from_snapshot(pool, user_id, snapshot, source_hash)
}

pub fn rebuild(pool: &DbPool, user_id: &str) -> Option<CachedProfileSummary> {
    let snapshot = build_snapshot(pool, user_id);
    if snapshot.terms.is_empty()
        && snapshot.aliases.is_empty()
        && snapshot.recent_outputs.is_empty()
    {
        return None;
    }
    let source_json = serde_json::to_string(&snapshot).ok()?;
    let source_hash = hash(&source_json);
    rebuild_from_snapshot(pool, user_id, snapshot, source_hash)
}

fn rebuild_from_snapshot(
    pool: &DbPool,
    user_id: &str,
    snapshot: SourceSnapshot,
    source_hash: String,
) -> Option<CachedProfileSummary> {
    let markdown = render_profile_markdown(&snapshot);
    if markdown.trim().is_empty() {
        return None;
    }
    let counts_json = serde_json::json!({
        "terms": snapshot.terms.len(),
        "aliases": snapshot.aliases.len(),
        "recent_outputs": snapshot.recent_outputs.len(),
    })
    .to_string();
    let now = now_ms();
    let conn = pool.get().ok()?;
    if let Err(e) = conn.execute(
        "INSERT INTO local_profile_summary
            (user_id, profile_markdown, source_hash, source_counts_json, version, updated_at)
         VALUES (?1, ?2, ?3, ?4, 1, ?5)
         ON CONFLICT(user_id) DO UPDATE SET
            profile_markdown = excluded.profile_markdown,
            source_hash = excluded.source_hash,
            source_counts_json = excluded.source_counts_json,
            version = local_profile_summary.version + 1,
            updated_at = excluded.updated_at",
        params![user_id, markdown, source_hash, counts_json, now],
    ) {
        warn!("[profile-summary] upsert failed: {e}");
        return None;
    }
    let refreshed = get_cached(pool, user_id)?;
    info!(
        "[profile-summary] rebuilt version={} chars={} terms={} aliases={} recent={}",
        refreshed.version,
        refreshed.profile_markdown.chars().count(),
        snapshot.terms.len(),
        snapshot.aliases.len(),
        snapshot.recent_outputs.len(),
    );
    Some(refreshed)
}

fn build_snapshot(pool: &DbPool, user_id: &str) -> SourceSnapshot {
    let terms = vocabulary::top_terms(pool, user_id, MAX_TERMS)
        .into_iter()
        .map(|term| TermSnapshot {
            term: term.term,
            term_type: term.term_type,
            meaning: term.meaning,
            context: term.example_context,
        })
        .collect::<Vec<_>>();
    let aliases = stt_replacements::load_all(pool, user_id)
        .into_iter()
        .filter(|alias| alias.review_status != ReviewStatus::Blocked)
        .take(MAX_ALIASES)
        .map(|alias| AliasSnapshot {
            heard: alias.transcript_form,
            correct: alias.correct_form,
            status: alias.review_status.as_str().to_string(),
        })
        .collect::<Vec<_>>();
    SourceSnapshot {
        terms,
        aliases,
        recent_outputs: recent_final_texts(pool, user_id, MAX_RECENT_TEXTS),
    }
}

fn recent_final_texts(pool: &DbPool, user_id: &str, limit: usize) -> Vec<String> {
    let Ok(conn) = pool.get() else {
        return vec![];
    };
    let mut stmt = match conn.prepare(
        "SELECT COALESCE(final_text, polished_output, polished, transcript)
           FROM recordings
          WHERE user_id = ?1
            AND COALESCE(final_text, polished_output, polished, transcript) IS NOT NULL
            AND length(TRIM(COALESCE(final_text, polished_output, polished, transcript))) > 20
          ORDER BY timestamp_ms DESC
          LIMIT ?2",
    ) {
        Ok(stmt) => stmt,
        Err(_) => return vec![],
    };
    stmt.query_map(params![user_id, limit as i64], |row| {
        row.get::<_, String>(0)
    })
    .ok()
    .map(|rows| rows.filter_map(Result::ok).collect())
    .unwrap_or_default()
}

fn render_profile_markdown(snapshot: &SourceSnapshot) -> String {
    let domains = infer_domains(snapshot);
    let stable_terms = snapshot
        .terms
        .iter()
        .take(30)
        .map(|term| {
            let kind = term.term_type.as_deref().unwrap_or("term");
            match term.meaning.as_deref().filter(|m| !m.trim().is_empty()) {
                Some(meaning) => format!("- {} ({kind}): {}", term.term, truncate(meaning, 120)),
                None => format!("- {} ({kind})", term.term),
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let aliases = snapshot
        .aliases
        .iter()
        .take(40)
        .map(|alias| {
            format!(
                "- {:?} -> {} ({})",
                alias.heard, alias.correct, alias.status
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let recent = snapshot
        .recent_outputs
        .iter()
        .take(5)
        .map(|text| format!("- {}", truncate(text, 180)))
        .collect::<Vec<_>>()
        .join("\n");
    let markdown = format!(
        "Background:\n\
         User speaks mixed English/Hinglish and uses AirNote for practical work communication. \
         Infer only from approved local learning rows and recent corrected outputs.\n\n\
         Focus areas:\n\
         {}\n\n\
         Speech style:\n\
         Natural Hinglish with technical/business nouns mixed into Hindi sentence structure. \
         Preserve casual intent; do not over-corporatize unless the user asks for a professional rewrite.\n\n\
         Stable vocabulary:\n\
         {}\n\n\
         STT recovery:\n\
         {}\n\n\
         Recent context:\n\
         {}\n",
        domains,
        empty_block(&stable_terms),
        empty_block(&aliases),
        empty_block(&recent),
    );
    truncate(&markdown, MAX_PROFILE_CHARS)
}

fn infer_domains(snapshot: &SourceSnapshot) -> String {
    let corpus = snapshot
        .terms
        .iter()
        .map(|term| term.term.as_str())
        .chain(snapshot.recent_outputs.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    let mut areas = Vec::new();
    if contains_any(
        &corpus,
        &[
            "kafka",
            "zookeeper",
            "docker",
            "sqlite",
            "postgres",
            "sentry",
            "webhook",
            "api",
            "stt",
            "deepgram",
            "deepseek",
            "maverick",
            "scout",
        ],
    ) {
        areas.push("software engineering, AI/STT testing, runtime latency, observability, infra");
    }
    if contains_any(
        &corpus,
        &["seo", "google ads", "meta ads", "campaign", "cpa", "ctr"],
    ) {
        areas.push("SEO, Google/Meta ads, campaign performance");
    }
    if contains_any(
        &corpus,
        &[
            "inventory",
            "warehouse",
            "invoice",
            "purchase order",
            "stock",
        ],
    ) {
        areas.push("inventory, operations, finance reconciliation");
    }
    if contains_any(
        &corpus,
        &[
            "client", "business", "emiac", "macobs", "proposal", "approval",
        ],
    ) {
        areas.push("business operations, client communication, company-specific names");
    }
    if areas.is_empty() {
        "General business/work communication. Use approved vocabulary as soft hints only."
            .to_string()
    } else {
        areas.join("; ")
    }
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn empty_block(text: &str) -> String {
    if text.trim().is_empty() {
        "- No approved local evidence yet.".to_string()
    } else {
        text.to_string()
    }
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out = text
        .chars()
        .take(max_chars.saturating_sub(3))
        .collect::<String>();
    out.push_str("...");
    out
}

fn hash(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    #[test]
    fn profile_shape_contains_required_sections() {
        let snapshot = super::SourceSnapshot {
            terms: vec![super::TermSnapshot {
                term: "Kafka".to_string(),
                term_type: Some("brand".to_string()),
                meaning: None,
                context: Some("Kafka retry issue".to_string()),
            }],
            aliases: vec![super::AliasSnapshot {
                heard: "kaafka".to_string(),
                correct: "Kafka".to_string(),
                status: "approved".to_string(),
            }],
            recent_outputs: vec!["Kafka aur ZooKeeper issue debug karna hai.".to_string()],
        };
        let markdown = super::render_profile_markdown(&snapshot);
        for section in [
            "Background:",
            "Focus areas:",
            "Speech style:",
            "Stable vocabulary:",
            "STT recovery:",
            "Recent context:",
        ] {
            assert!(markdown.contains(section), "missing {section}");
        }
        assert!(markdown.contains("Kafka"));
        assert!(markdown.contains("kaafka"));
    }
}
