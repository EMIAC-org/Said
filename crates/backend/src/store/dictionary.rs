//! The user's word list: where the transcript has `heard`, write `written`.
//!
//! Entries come from two places — words the user adds on the Dictionary page,
//! and words learned when the user corrects what AirNote typed. The only
//! reader is polish: the entries that occur in a transcript go into its
//! prompt. With polish off nothing reads them.

use rusqlite::{OptionalExtension, params};
use said_core::polish::dictation::DictionaryEntry;
use serde::Serialize;
use tracing::{info, warn};

use super::{DbPool, now_ms};

pub const SOURCE_LEARNED: &str = "learned";
pub const SOURCE_ADDED: &str = "added";

const MAX_PHRASE_WORDS: usize = 3;
const MAX_CHANGED_REGIONS: usize = 3;
/// Past this many word pairs the diff is not worth computing for a dictation.
const MAX_DIFF_CELLS: usize = 400_000;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Entry {
    pub id: i64,
    pub written: String,
    pub heard: Option<String>,
    pub source: String,
    pub created_at: i64,
}

pub fn list(pool: &DbPool, user_id: &str) -> Vec<Entry> {
    let Ok(conn) = pool.get() else {
        return vec![];
    };
    let Ok(mut stmt) = conn.prepare(
        "SELECT id, written, heard, source, created_at
           FROM dictionary
          WHERE user_id = ?1
          ORDER BY created_at DESC, id DESC",
    ) else {
        return vec![];
    };
    stmt.query_map([user_id], |row| {
        Ok(Entry {
            id: row.get(0)?,
            written: row.get(1)?,
            heard: row.get(2)?,
            source: row.get(3)?,
            created_at: row.get(4)?,
        })
    })
    .map(|rows| rows.filter_map(Result::ok).collect())
    .unwrap_or_default()
}

/// Add an entry, or return the existing one with the same spelling and
/// misheard form. `None` when `written` is blank or the write fails.
pub fn add(
    pool: &DbPool,
    user_id: &str,
    written: &str,
    heard: Option<&str>,
    source: &str,
) -> Option<Entry> {
    let written = one_line(written);
    let heard = heard.map(one_line).filter(|h| !h.is_empty());
    if written.is_empty() {
        return None;
    }
    let conn = pool.get().ok()?;
    let now = now_ms();
    if let Err(e) = conn.execute(
        "INSERT INTO dictionary (user_id, written, heard, source, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)
         ON CONFLICT DO UPDATE SET updated_at = excluded.updated_at",
        params![user_id, written, heard, source, now],
    ) {
        warn!("[dictionary] add {written:?} failed: {e}");
        return None;
    }
    conn.query_row(
        "SELECT id, written, heard, source, created_at
           FROM dictionary
          WHERE user_id = ?1 AND lower(written) = lower(?2)
            AND lower(coalesce(heard, '')) = lower(coalesce(?3, ''))",
        params![user_id, written, heard],
        |row| {
            Ok(Entry {
                id: row.get(0)?,
                written: row.get(1)?,
                heard: row.get(2)?,
                source: row.get(3)?,
                created_at: row.get(4)?,
            })
        },
    )
    .optional()
    .ok()
    .flatten()
}

pub fn delete(pool: &DbPool, user_id: &str, id: i64) -> bool {
    pool.get()
        .ok()
        .and_then(|conn| {
            conn.execute(
                "DELETE FROM dictionary WHERE user_id = ?1 AND id = ?2",
                params![user_id, id],
            )
            .ok()
        })
        .is_some_and(|n| n > 0)
}

pub fn delete_all(pool: &DbPool, user_id: &str) -> usize {
    pool.get()
        .ok()
        .and_then(|conn| {
            conn.execute("DELETE FROM dictionary WHERE user_id = ?1", [user_id])
                .ok()
        })
        .unwrap_or(0)
}

/// The entries polish should see for this transcript: those whose misheard
/// form — or, for a bare spelling, the word itself — occurs in it.
pub fn for_transcript(pool: &DbPool, user_id: &str, transcript: &str) -> Vec<DictionaryEntry> {
    let transcript_words = phrase_words(transcript);
    list(pool, user_id)
        .into_iter()
        .filter(|entry| {
            let cue = entry.heard.as_deref().unwrap_or(&entry.written);
            contains_words(&transcript_words, &phrase_words(cue))
        })
        .map(|entry| DictionaryEntry {
            heard: entry.heard,
            written: entry.written,
        })
        .collect()
}

/// Learn the words the user fixed when they edited `typed` into `kept`.
/// Returns the entries that are new to the list.
pub fn learn_from_edit(pool: &DbPool, user_id: &str, typed: &str, kept: &str) -> Vec<Entry> {
    let known = list(pool, user_id);
    let mut learned = Vec::new();
    for (heard, written) in learned_pairs(typed, kept) {
        let already_known = known.iter().any(|e| {
            e.written == written
                && e.heard
                    .as_deref()
                    .is_some_and(|h| h.eq_ignore_ascii_case(&heard))
        });
        if already_known {
            continue;
        }
        if let Some(entry) = add(pool, user_id, &written, Some(&heard), SOURCE_LEARNED) {
            info!("[dictionary] learned {heard:?} → {written:?}");
            learned.push(entry);
        }
    }
    learned
}

/// Word swaps worth learning from one edit: small replacements (up to three
/// words each side) whose new text looks like a name or term — it has a
/// capital letter, a digit or a symbol such as `.` or `-`. Case-only changes
/// count only when the capitals are inside the word (`airnote` → `AirNote`),
/// so capitalizing a sentence start teaches nothing. An edit that changes
/// more than half the words, or more than three places, is a rewrite and
/// teaches nothing either.
pub fn learned_pairs(typed: &str, kept: &str) -> Vec<(String, String)> {
    let typed: Vec<String> = typed.split_whitespace().map(strip_edges).collect();
    let kept: Vec<String> = kept.split_whitespace().map(strip_edges).collect();
    if typed.is_empty() || kept.is_empty() || typed.len() * kept.len() > MAX_DIFF_CELLS {
        return vec![];
    }

    let regions = changed_regions(&typed, &kept);
    let changed_words: usize = regions.iter().map(|r| r.typed.len()).sum();
    if regions.len() > MAX_CHANGED_REGIONS || changed_words * 2 > typed.len() {
        return vec![];
    }

    regions
        .into_iter()
        .filter(|r| {
            (1..=MAX_PHRASE_WORDS).contains(&r.typed.len())
                && (1..=MAX_PHRASE_WORDS).contains(&r.kept.len())
        })
        .filter_map(|r| {
            let heard = r.typed.join(" ").to_lowercase();
            let written = r.kept.join(" ");
            if heard.is_empty() || written.is_empty() || !looks_like_a_term(&written) {
                return None;
            }
            let case_only = heard == written.to_lowercase();
            if case_only && !has_inner_capital(&written) {
                return None;
            }
            Some((heard, written))
        })
        .collect()
}

struct Region {
    typed: Vec<String>,
    kept: Vec<String>,
}

/// The places where `kept` differs from `typed`, by longest common subsequence
/// of words.
fn changed_regions(typed: &[String], kept: &[String]) -> Vec<Region> {
    let (n, m) = (typed.len(), kept.len());
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if typed[i] == kept[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }

    let mut regions = Vec::new();
    let (mut i, mut j) = (0, 0);
    let mut current = Region {
        typed: vec![],
        kept: vec![],
    };
    while i < n || j < m {
        if i < n && j < m && typed[i] == kept[j] {
            if !current.typed.is_empty() || !current.kept.is_empty() {
                regions.push(std::mem::replace(
                    &mut current,
                    Region {
                        typed: vec![],
                        kept: vec![],
                    },
                ));
            }
            i += 1;
            j += 1;
        } else if j < m && (i == n || lcs[i][j + 1] >= lcs[i + 1][j]) {
            current.kept.push(kept[j].clone());
            j += 1;
        } else {
            current.typed.push(typed[i].clone());
            i += 1;
        }
    }
    if !current.typed.is_empty() || !current.kept.is_empty() {
        regions.push(current);
    }
    regions
}

fn looks_like_a_term(text: &str) -> bool {
    text.chars().any(|c| {
        c.is_uppercase()
            || c.is_ascii_digit()
            || matches!(c, '.' | '-' | '_' | '@' | '/' | '#' | '+' | '&')
    })
}

fn has_inner_capital(text: &str) -> bool {
    text.split_whitespace()
        .any(|word| word.chars().skip(1).any(char::is_uppercase))
}

/// Punctuation at the edges of a word is sentence punctuation, not part of it.
fn strip_edges(word: &str) -> String {
    word.trim_matches(|c: char| !c.is_alphanumeric())
        .to_string()
}

fn phrase_words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|w| strip_edges(w).to_lowercase())
        .filter(|w| !w.is_empty())
        .collect()
}

fn contains_words(haystack: &[String], needle: &[String]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(typed: &str, kept: &str) -> Vec<(String, String)> {
        learned_pairs(typed, kept)
    }

    fn pair(heard: &str, written: &str) -> (String, String) {
        (heard.to_string(), written.to_string())
    }

    #[test]
    fn a_split_brand_name_is_learned() {
        assert_eq!(
            pairs(
                "bhai air note ka build bhejo",
                "bhai AirNote ka build bhejo"
            ),
            vec![pair("air note", "AirNote")]
        );
    }

    #[test]
    fn a_misheard_name_is_learned() {
        assert_eq!(
            pairs("large par message bhejo.", "Lark par message bhejo."),
            vec![pair("large", "Lark")]
        );
        assert_eq!(
            pairs("deploy dev b1 today", "deploy dev-v1 today"),
            vec![pair("dev b1", "dev-v1")]
        );
    }

    #[test]
    fn capitals_inside_a_word_are_learned_but_a_capitalized_sentence_start_is_not() {
        assert_eq!(
            pairs("the emiac team", "the EMIAC team"),
            vec![pair("emiac", "EMIAC")]
        );
        assert!(pairs("hello bhai kaise ho", "Hello bhai kaise ho").is_empty());
    }

    #[test]
    fn plain_word_changes_and_punctuation_teach_nothing() {
        assert!(pairs("isko kar do abhi", "isko karna abhi").is_empty());
        assert!(pairs("haan theek hai", "haan, theek hai.").is_empty());
        assert!(pairs("send it now", "send it now please").is_empty());
    }

    #[test]
    fn a_rewrite_teaches_nothing() {
        assert!(
            pairs(
                "can we close the design review today",
                "Let's wrap up the Figma review tomorrow"
            )
            .is_empty()
        );
    }

    #[test]
    fn long_replacements_teach_nothing() {
        assert!(
            pairs(
                "one two three four five six seven eight nine ten",
                "one Alpha Beta Gamma Delta six seven eight nine ten"
            )
            .is_empty()
        );
    }

    #[test]
    fn phrases_match_whole_words_ignoring_case_and_punctuation() {
        let transcript = phrase_words("Bhai, air note ka build bhejo.");
        assert!(contains_words(&transcript, &phrase_words("air note")));
        assert!(contains_words(&transcript, &phrase_words("Air Note")));
        assert!(!contains_words(&transcript, &phrase_words("airnote")));
        assert!(!contains_words(
            &transcript,
            &phrase_words("note ka build bhejo now")
        ));
        assert!(!contains_words(
            &phrase_words("airnotes"),
            &phrase_words("air note")
        ));
    }

    fn pool() -> DbPool {
        let dir = std::env::temp_dir().join(format!("airnote-dictionary-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        crate::store::open(&dir.join("said.db"))
    }

    #[test]
    fn learned_words_reach_only_the_transcripts_that_contain_them() {
        let pool = pool();
        let user = crate::store::ensure_default_user(&pool);

        let learned = learn_from_edit(
            &pool,
            &user,
            "air note ka build bhejo",
            "AirNote ka build bhejo",
        );
        assert_eq!(learned.len(), 1);
        add(&pool, &user, "EMIAC", None, SOURCE_ADDED).unwrap();

        assert_eq!(
            for_transcript(&pool, &user, "Air note ka naya build"),
            vec![DictionaryEntry {
                heard: Some("air note".into()),
                written: "AirNote".into(),
            }]
        );
        assert_eq!(
            for_transcript(&pool, &user, "emiac ki team"),
            vec![DictionaryEntry {
                heard: None,
                written: "EMIAC".into(),
            }]
        );
        assert!(for_transcript(&pool, &user, "kal milte hain").is_empty());
    }

    #[test]
    fn learning_the_same_fix_twice_keeps_one_entry() {
        let pool = pool();
        let user = crate::store::ensure_default_user(&pool);
        assert_eq!(
            learn_from_edit(&pool, &user, "large par", "Lark par").len(),
            1
        );
        assert!(learn_from_edit(&pool, &user, "large mein", "Lark mein").is_empty());
        assert_eq!(list(&pool, &user).len(), 1);
    }

    #[test]
    fn entries_can_be_deleted() {
        let pool = pool();
        let user = crate::store::ensure_default_user(&pool);
        let entry = add(&pool, &user, "Clario", Some("claryo"), SOURCE_ADDED).unwrap();
        add(&pool, &user, "  ", None, SOURCE_ADDED);
        assert_eq!(list(&pool, &user), vec![entry.clone()]);
        assert!(delete(&pool, &user, entry.id));
        assert!(list(&pool, &user).is_empty());
    }
}
