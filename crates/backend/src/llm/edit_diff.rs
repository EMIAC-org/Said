//! Deterministic structural diff between (transcript, polish, user_kept).
//!
//! This is the **foundation** of the learning pipeline.  Every learnable
//! candidate must be backed by real text from the transcript, AirNote output,
//! or user-kept text.  A narrow LLM fallback may propose spans for complex
//! edits, but the classifier verifies those spans against this diff/input
//! before anything can be learned.  That architectural decision eliminates
//! by construction the entire class of hallucination bugs (e.g. proposing
//! Devanagari "corrections" the user never typed).
//!
//! Algorithm:
//!   1. Tokenise polish and user_kept into whitespace-delimited tokens.
//!   2. Compute the longest common subsequence (LCS) over those tokens.
//!   3. Walk the LCS to produce a list of `Hunk`s — each hunk is one
//!      contiguous run of "polish had X, user kept Y" (where either side
//!      may be empty for pure insertions/deletions).
//!   4. For each hunk, locate the corresponding window in the transcript
//!      by aligning the hunk's polish-side token positions against the
//!      transcript's tokens.  We use a simple positional align — if the
//!      transcript and polish have the same token count, position N in one
//!      maps to position N in the other.  Otherwise we record the
//!      transcript window as the full transcript (the LLM will
//!      disambiguate).
//!
//! Output is a `Vec<Hunk>` ready to hand to the classifier as the fixed
//! evidence set.  The LLM may only help interpret complex edits; it cannot
//! fabricate learnable spans outside the transcript/output/kept evidence.

use serde::{Deserialize, Serialize};

/// One structural difference between polish and user_kept.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Hunk {
    /// Token slice from the original transcript that corresponds positionally
    /// to this hunk's polish window.  Empty if no positional mapping exists.
    pub transcript_window: String,
    /// What the polish step produced for this region.  Empty for pure
    /// insertions (user added words that were never in the polish).
    pub polish_window: String,
    /// What the user actually kept for this region.  Empty for pure
    /// deletions (user removed words that were in the polish).
    pub kept_window: String,
}

/// Compute the structural diff.  Returns at most a few hunks for typical
/// edits; returns an empty vec if polish == user_kept after trimming.
pub fn diff(transcript: &str, polish: &str, user_kept: &str) -> Vec<Hunk> {
    let p_tokens: Vec<&str> = polish.split_whitespace().collect();
    let k_tokens: Vec<&str> = user_kept.split_whitespace().collect();
    let t_tokens: Vec<&str> = transcript.split_whitespace().collect();

    if p_tokens == k_tokens {
        return Vec::new();
    }

    // ── 1. LCS table over polish vs user_kept tokens ──────────────────────────
    let n = p_tokens.len();
    let m = k_tokens.len();
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in 0..n {
        for j in 0..m {
            if p_tokens[i] == k_tokens[j] {
                lcs[i + 1][j + 1] = lcs[i][j] + 1;
            } else {
                lcs[i + 1][j + 1] = lcs[i + 1][j].max(lcs[i][j + 1]);
            }
        }
    }

    // ── 2. Backtrack into operations: Equal | Replace | Insert | Delete ──────
    let mut ops: Vec<Op> = Vec::new();
    let mut i = n;
    let mut j = m;
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && p_tokens[i - 1] == k_tokens[j - 1] {
            ops.push(Op::Equal(p_tokens[i - 1].to_string()));
            i -= 1;
            j -= 1;
        } else if j > 0 && (i == 0 || lcs[i][j - 1] >= lcs[i - 1][j]) {
            ops.push(Op::Insert(k_tokens[j - 1].to_string()));
            j -= 1;
        } else {
            ops.push(Op::Delete(p_tokens[i - 1].to_string()));
            i -= 1;
        }
    }
    ops.reverse();

    // ── 3. Coalesce consecutive non-equal ops into hunks; track polish offset
    //      so we can pick the corresponding transcript window. ───────────────
    let positional_align = t_tokens.len() == p_tokens.len();
    let mut hunks: Vec<Hunk> = Vec::new();
    let mut polish_offset = 0_usize;
    let mut current_polish: Vec<String> = Vec::new();
    let mut current_kept: Vec<String> = Vec::new();
    let mut hunk_polish_start = 0_usize;

    let flush = |hunks: &mut Vec<Hunk>,
                 current_polish: &mut Vec<String>,
                 current_kept: &mut Vec<String>,
                 hunk_polish_start: usize,
                 polish_offset: usize,
                 t_tokens: &[&str],
                 positional_align: bool| {
        if current_polish.is_empty() && current_kept.is_empty() {
            return;
        }
        let polish_window = current_polish.join(" ");
        let kept_window = current_kept.join(" ");
        let transcript_window = if positional_align {
            t_tokens[hunk_polish_start..polish_offset].join(" ")
        } else {
            String::new() // unaligned — let the LLM see it as missing
        };
        hunks.push(Hunk {
            transcript_window,
            polish_window,
            kept_window,
        });
        current_polish.clear();
        current_kept.clear();
    };

    for op in &ops {
        match op {
            Op::Equal(_) => {
                flush(
                    &mut hunks,
                    &mut current_polish,
                    &mut current_kept,
                    hunk_polish_start,
                    polish_offset,
                    &t_tokens,
                    positional_align,
                );
                polish_offset += 1;
                hunk_polish_start = polish_offset;
            }
            Op::Delete(p) => {
                if current_polish.is_empty() && current_kept.is_empty() {
                    hunk_polish_start = polish_offset;
                }
                current_polish.push(p.clone());
                polish_offset += 1;
            }
            Op::Insert(k) => {
                if current_polish.is_empty() && current_kept.is_empty() {
                    hunk_polish_start = polish_offset;
                }
                current_kept.push(k.clone());
            }
        }
    }
    flush(
        &mut hunks,
        &mut current_polish,
        &mut current_kept,
        hunk_polish_start,
        polish_offset,
        &t_tokens,
        positional_align,
    );

    hunks
}

/// Diff operation in the polish→user_kept rewrite.
#[derive(Debug)]
enum Op {
    Equal(String),
    Delete(String), // present in polish, absent in user_kept
    Insert(String), // absent from polish, present in user_kept
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_diff_returns_empty() {
        let hunks = diff("hello world", "hello world", "hello world");
        assert!(hunks.is_empty());
    }

    #[test]
    fn single_word_substitution() {
        // The canonical n8n case.
        let hunks = diff(
            "i use written for automation",
            "I use written for automation",
            "I use n8n for automation",
        );
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].polish_window, "written");
        assert_eq!(hunks[0].kept_window, "n8n");
        assert_eq!(hunks[0].transcript_window, "written");
    }

    #[test]
    fn pure_prefix_insertion_yields_one_hunk_with_empty_polish() {
        // The email-link bug case: user added a markdown link before the polish.
        let polish = "Anish at Gmail dot com ka zara batana";
        let kept =
            "[anish@gmail.com](mailto:anish@gmail.com) Anish at Gmail dot com ka zara batana";
        let hunks = diff(polish, polish, kept);
        assert_eq!(hunks.len(), 1, "should produce exactly one insertion hunk");
        assert_eq!(hunks[0].polish_window, "");
        assert!(hunks[0].kept_window.starts_with("["));
        // The hallucinated "अनीष / का / ज़रा" candidates from the LLM bug
        // CANNOT appear here because they aren't in the actual text.
        assert!(!hunks[0].kept_window.contains("अनीष"));
        assert!(!hunks[0].kept_window.contains("का"));
    }

    #[test]
    fn deletion_yields_hunk_with_empty_kept() {
        let hunks = diff("hello big world", "hello big world", "hello world");
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].polish_window, "big");
        assert_eq!(hunks[0].kept_window, "");
    }

    #[test]
    fn multiple_separated_substitutions_produce_multiple_hunks() {
        let polish = "the quick brown fox jumps";
        let kept = "a quick red fox runs";
        let hunks = diff(polish, polish, kept);
        assert!(hunks.len() >= 2, "expected multiple hunks, got: {hunks:?}");
    }

    #[test]
    fn transcript_window_matches_when_token_counts_align() {
        // Same token count between transcript & polish → positional align works.
        let hunks = diff(
            "i use written daily",
            "I use written daily",
            "I use n8n daily",
        );
        assert_eq!(hunks[0].transcript_window, "written");
    }

    #[test]
    fn no_candidate_fabrication_in_devanagari_bug_case() {
        // The exact bug from the user's logs: the LLM hallucinated Devanagari
        // candidates that weren't in any of the three texts.  Diff-based
        // candidates can ONLY come from the texts themselves, so the bad
        // candidates are unreachable by construction.
        let transcript = "Anish at Gmail dot com ka zara batana kaun sa mail ID par bhejna hai";
        let polish = "Anish at Gmail dot com ka zara batana kaun sa mail ID par bhejna hai";
        let kept = "[anish@gmail.com](mailto:anish@gmail.com) Anish at Gmail dot com ka zara batana kaun sa mail ID par bhejna hai";
        let hunks = diff(transcript, polish, kept);
        for h in &hunks {
            assert!(!h.kept_window.contains("अनीष"));
            assert!(!h.kept_window.contains("का"));
            assert!(!h.kept_window.contains("ज़रा"));
        }
    }
}
