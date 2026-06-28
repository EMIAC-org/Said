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
//!   4. For each hunk, recover the corresponding transcript window by locating
//!      the hunk's polish-side tokens as a UNIQUE contiguous run inside the
//!      transcript (per-hunk, content-based — see `find_unique_window`). This
//!      replaced an older single global token-count alignment that blanked the
//!      window for the whole utterance whenever one unrelated edit changed the
//!      total token count. Pure insertions, and absent or ambiguous runs, leave
//!      the window empty rather than guess.
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

    // ── 3. Coalesce consecutive non-equal ops into hunks. Each hunk's transcript
    //      window is recovered PER-HUNK by content (find_unique_window), not by a
    //      single global token-count flag — so one unrelated count-changing edit
    //      elsewhere in the utterance can no longer blank a clean 1:1 alias hunk's
    //      transcript evidence (the audit's empty-window defect). ───────────────
    let mut hunks: Vec<Hunk> = Vec::new();
    let mut current_polish: Vec<String> = Vec::new();
    let mut current_kept: Vec<String> = Vec::new();

    let flush = |hunks: &mut Vec<Hunk>,
                 current_polish: &mut Vec<String>,
                 current_kept: &mut Vec<String>,
                 t_tokens: &[&str]| {
        if current_polish.is_empty() && current_kept.is_empty() {
            return;
        }
        let polish_window = current_polish.join(" ");
        let kept_window = current_kept.join(" ");
        // Find this hunk's polish tokens as a unique contiguous run in the
        // transcript. Empty for pure insertions (no polish window) or when the
        // run is absent/ambiguous — never invents text outside the transcript.
        let transcript_window = find_unique_window(t_tokens, current_polish);
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
                    &t_tokens,
                );
            }
            Op::Delete(p) => {
                current_polish.push(p.clone());
            }
            Op::Insert(k) => {
                current_kept.push(k.clone());
            }
        }
    }
    flush(
        &mut hunks,
        &mut current_polish,
        &mut current_kept,
        &t_tokens,
    );

    split_aligned_substitutions(hunks)
}

/// Locate `polish` as a unique contiguous (ASCII-case-insensitive) run inside the
/// transcript tokens `t` and return it joined by spaces. Returns "" if `polish`
/// is empty (a pure insertion), absent, or occurs more than once (ambiguous) — so
/// it only ever FILLS a window that positional alignment would have wrongly
/// blanked, and never returns text outside `t` (preserving the evidence-bound
/// invariant that makes hallucinated candidates unreachable by construction).
fn find_unique_window(t: &[&str], polish: &[String]) -> String {
    let n = polish.len();
    if n == 0 || t.len() < n {
        return String::new();
    }
    let mut found: Option<usize> = None;
    for start in 0..=(t.len() - n) {
        if (0..n).all(|k| t[start + k].eq_ignore_ascii_case(&polish[k])) {
            if found.is_some() {
                return String::new(); // ambiguous — leave empty rather than guess
            }
            found = Some(start);
        }
    }
    found.map(|s| t[s..s + n].join(" ")).unwrap_or_default()
}

/// Split a contiguous equal-count substitution blob into one hunk per changed
/// token, so high replacement counts are each identified individually rather
/// than collapsed into one unlearnable multi-word blob.
///
/// Only fires when the polish and kept windows have the SAME token count (> 1):
/// that's an unambiguous 1:1 positional rewrite (e.g. "a b c" → "x y z" → three
/// swaps). Windows of differing length (true phrase rewrites like a whole-
/// sentence replacement) are left intact.
fn split_aligned_substitutions(hunks: Vec<Hunk>) -> Vec<Hunk> {
    let mut out = Vec::with_capacity(hunks.len());
    for h in hunks {
        let p: Vec<&str> = h.polish_window.split_whitespace().collect();
        let k: Vec<&str> = h.kept_window.split_whitespace().collect();
        let t: Vec<&str> = h.transcript_window.split_whitespace().collect();
        if p.len() > 1 && p.len() == k.len() {
            let t_aligned = t.len() == p.len();
            for i in 0..p.len() {
                if p[i] == k[i] {
                    continue; // token unchanged inside the blob
                }
                out.push(Hunk {
                    transcript_window: if t_aligned {
                        t[i].to_string()
                    } else {
                        String::new()
                    },
                    polish_window: p[i].to_string(),
                    kept_window: k[i].to_string(),
                });
            }
        } else {
            out.push(h);
        }
    }
    out
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
    fn aligned_multiword_substitution_splits_per_token() {
        // High replacement count: a contiguous N:N rewrite must yield one hunk
        // per changed word, so each swap is identified instead of one blob.
        let hunks = diff(
            "the quick brown fox",
            "the quick brown fox",
            "the slow red cat",
        );
        assert_eq!(hunks.len(), 3, "expected per-word hunks, got: {hunks:?}");
        assert_eq!(hunks[0].polish_window, "quick");
        assert_eq!(hunks[0].kept_window, "slow");
        assert_eq!(hunks[1].polish_window, "brown");
        assert_eq!(hunks[1].kept_window, "red");
        assert_eq!(hunks[2].polish_window, "fox");
        assert_eq!(hunks[2].kept_window, "cat");
    }

    #[test]
    fn unequal_length_replacement_stays_one_hunk() {
        // A true phrase rewrite (different token counts) must NOT be split into
        // bogus per-word swaps — it stays one graceful change.
        let hunks = diff(
            "let us meet tomorrow",
            "let us meet tomorrow",
            "cancel it now",
        );
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].kept_window, "cancel it now");
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

/// Real-world Hinglish dictation-edit corpus (built from the user's screenshot +
/// realistic STT/polish/style cases). Asserts the REAL edit_diff::diff output and
/// the promotion-gate common-word gate, so transcript-window recovery and the
/// no-learn safety net are locked against regressions.
#[cfg(test)]
mod fixture_corpus_tests {
    use crate::llm::{edit_diff, promotion_gate};

    fn one_changed_hunk(t: &str, p: &str, k: &str) -> edit_diff::Hunk {
        let hunks = edit_diff::diff(t, p, k);
        assert_eq!(
            hunks.len(),
            1,
            "expected one hunk for {p:?} -> {k:?}, got {hunks:?}"
        );
        hunks.into_iter().next().unwrap()
    }

    // FIXTURE 1 — the screenshot. A phrase rephrase that must NOT learn an alias.
    #[test]
    fn fixture1_screenshot_phrase_rewrite_is_style_preference_no_alias() {
        let transcript = "Bhai windows vaala to yah rahe hain koi dekhana padega. Mere ko to kuchh samajh nahin aa raha. Yah jo windows hai na yah kaaphi puraana hai mere hisaab se to ismen to nahin hi chal raha hai. Aur kyonki keys ka conflict hai. Device yahaan par Mac hai aur software windows hai to vo keys ka conflict ke kaaran kuchh dikkat aa raha hai mere hisaab se.";
        let polish = "Bhai, windows vaala to yah rahe hain, koi dekhna padega. Mere ko to kuchh samajh nahin aa raha. Yah jo windows hai na, kaafi purana hai mere hisaab se, ismein to nahin hi chal raha hai. Aur kyunki keys ka conflict hai. Device yahaan par Mac hai aur software Windows hai, to vo keys ka conflict ke kaaran kuchh dikkat aa raha hai mere hisaab se.";
        let kept = "Bhai, windows vaala to tujhe hi dekhna padega. Mere ko to kuchh samajh nahin aa raha. Yah jo windows hai na, kaafi purana hai mere hisaab se, ismein to nahin hi chal raha hai. Aur kyunki keys ka conflict hai. Device yahaan par Mac hai aur software Windows hai, to vo keys ka conflict ke kaaran kuchh dikkat aa raha hai mere hisaab se.";
        let h = one_changed_hunk(transcript, polish, kept);
        assert_eq!(h.polish_window, "yah rahe hain, koi");
        assert_eq!(h.kept_window, "tujhe hi");
        // The ORIGINAL phrase is all-common -> classify resolves to StylePreference
        // (no alias). The corrected form is now ALSO covered by the deny-list (tu-
        // family added), giving defense-in-depth.
        assert!(promotion_gate::is_common_word(&h.polish_window));
        assert!(promotion_gate::is_common_word(&h.kept_window));
        // 4 -> 2 unequal length => not split into per-token swaps => one hunk.
        assert_ne!(
            h.polish_window.split_whitespace().count(),
            h.kept_window.split_whitespace().count()
        );
    }

    #[test]
    fn fixture2_deepgram_two_to_one_collapse_is_learnable_alias() {
        let h = one_changed_hunk(
            "hum log deep gram use karte hain streaming ke liye",
            "Hum log deep gram use karte hain streaming ke liye.",
            "Hum log Deepgram use karte hain streaming ke liye.",
        );
        assert_eq!(h.polish_window, "deep gram");
        assert_eq!(h.kept_window, "Deepgram");
        assert_eq!(h.transcript_window, "deep gram");
        assert!(!promotion_gate::is_common_word(&h.kept_window));
    }

    #[test]
    fn fixture3_supabase_two_to_one_collapse() {
        let h = one_changed_hunk(
            "saara data super base mein store hota hai",
            "Saara data super base mein store hota hai.",
            "Saara data Supabase mein store hota hai.",
        );
        assert_eq!(h.polish_window, "super base");
        assert_eq!(h.kept_window, "Supabase");
        assert_eq!(h.transcript_window, "super base");
    }

    #[test]
    fn fixture4_n8n_canonical_alias() {
        let h = one_changed_hunk(
            "automation ke liye main n 10 use karta hoon",
            "Automation ke liye main n 10 use karta hoon",
            "Automation ke liye main n8n use karta hoon",
        );
        assert_eq!(h.polish_window, "n 10");
        assert_eq!(h.kept_window, "n8n");
        assert_eq!(h.transcript_window, "n 10");
        assert!(!promotion_gate::is_numeric_junk(&h.kept_window));
        assert!(!promotion_gate::is_common_word(&h.kept_window));
    }

    #[test]
    fn fixture11_identical_after_norm_yields_no_hunk() {
        let hunks = edit_diff::diff(
            "toh kal milte hain office mein",
            "Toh kal milte hain office mein.",
            "Toh kal milte hain office mein.",
        );
        assert!(hunks.is_empty());
    }

    #[test]
    fn fixture12_casing_only_tweaks_are_not_real_word_changes() {
        let hunks = edit_diff::diff(
            "mac par windows ka conflict aa raha hai",
            "mac par windows ka conflict aa raha hai.",
            "Mac par Windows ka conflict aa raha hai.",
        );
        for h in &hunks {
            assert_eq!(
                h.polish_window.to_ascii_lowercase(),
                h.kept_window.to_ascii_lowercase(),
                "casing-only hunk: {h:?}"
            );
        }
    }

    #[test]
    fn fixture13_multibrand_separates_into_two_hunks() {
        let hunks = edit_diff::diff(
            "hum vector ko super base aur n 10 dono mein test karenge",
            "Hum vector ko super base aur n 10 dono mein test karenge.",
            "Hum vector ko Supabase aur n8n dono mein test karenge.",
        );
        assert_eq!(hunks.len(), 2, "separated by 'aur': {hunks:?}");
        assert!(hunks.iter().any(|h| h.kept_window == "Supabase"));
        assert!(hunks.iter().any(|h| h.kept_window == "n8n"));
    }

    #[test]
    fn fixture14_long_sentence_single_brand_fix_keeps_transcript_window() {
        // transcript==polish in token count here, so this passed even before the
        // per-hunk fix — keep it as a positive control.
        let h = one_changed_hunk(
            "aaj ke sprint mein humein dictation pipeline ke andar deep gram wali streaming latency ko theek karna hai warna release slip ho jayega",
            "Aaj ke sprint mein humein dictation pipeline ke andar deep gram wali streaming latency ko theek karna hai, warna release slip ho jayega.",
            "Aaj ke sprint mein humein dictation pipeline ke andar Deepgram wali streaming latency ko theek karna hai, warna release slip ho jayega.",
        );
        assert_eq!(h.kept_window, "Deepgram");
        assert_eq!(h.transcript_window, "deep gram");
    }

    #[test]
    fn fixture16_inserted_email_prefix_is_pure_insertion() {
        let hunks = edit_diff::diff(
            "anish ko mail kar dena report ke baare mein",
            "Anish ko mail kar dena report ke baare mein.",
            "anish@gmail.com Anish ko mail kar dena report ke baare mein.",
        );
        assert!(
            hunks
                .iter()
                .any(|h| h.polish_window.trim().is_empty()
                    && h.kept_window.contains("anish@gmail.com")),
            "expected pure-insertion hunk: {hunks:?}"
        );
    }

    // The per-hunk-alignment fix in action: an unrelated early token-count change
    // (polish drops the repeated "yaar") used to blank the transcript window for
    // the WHOLE utterance (global positional_align=false). With per-hunk content
    // lookup, the clean "deep gram" alias hunk keeps its transcript evidence.
    #[test]
    fn fixture17_per_hunk_alignment_recovers_clean_alias_window() {
        let transcript = "yaar yeh kaam bahut zyada important hai aur humein deep gram wali latency theek karni hai";
        let polish =
            "Yeh kaam bahut zyada important hai aur humein deep gram wali latency theek karni hai.";
        let kept =
            "Yeh kaam bahut zyada important hai aur humein Deepgram wali latency theek karni hai.";
        let hunks = edit_diff::diff(transcript, polish, kept);
        let h = hunks
            .iter()
            .find(|h| h.kept_window == "Deepgram")
            .expect("alias hunk");
        // FIXED: previously "" (blanked by the global flag); now recovered.
        assert_eq!(h.transcript_window, "deep gram");
    }
}
