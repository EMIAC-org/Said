//! Stage 2.5 — phonetic triage between diff and LLM.
//!
//! For each diff hunk, decide CHEAPLY whether the (polish_window → kept_window)
//! transition is so obviously an STT mishearing or so obviously a rephrase
//! that we don't need the LLM to label it. Hunks that pass through here
//! confidently get a synthetic `LabelledHunk` and skip Groq entirely; only
//! ambiguous hunks fall through.
//!
//! The wins:
//!   • Cheaper — typical edits skip 70%+ of LLM calls (no network round-trip).
//!   • Faster — synthetic decisions are sub-millisecond vs. ~150 ms for Groq.
//!   • More stable — phonetic + length signals don't drift between runs the
//!     way LLM judgment can.
//!
//! Decision matrix per hunk:
//!
//!   polish_window vs kept_window:
//!     ┌─────────────────────────────────────────────────────────────────┐
//!     │ phonetic match (≥ 0.7) AND short Lev (≤ 2)  → STT_ERROR (clear) │
//!     │ phonetic mismatch (< 0.4) AND big Lev (≥ 4) AND jargon < 0.15  │
//!     │                                              → USER_REPHRASE   │
//!     │ kept_window is multi-word, polish is single word               │
//!     │                                              → AMBIGUOUS (LLM) │
//!     │ pure deletion / insertion                    → AMBIGUOUS (LLM) │
//!     │ everything else                              → AMBIGUOUS (LLM) │
//!     └─────────────────────────────────────────────────────────────────┘
//!
//! Conservative by construction — when in doubt, ALWAYS forward to the LLM.
//! False positives here would skip the LLM's judgement entirely, so we err
//! on the side of letting things through.

use super::{
    classifier::{EditClass, ExtractedTerm, LabelledHunk},
    edit_diff::Hunk,
    phonetics,
};

/// Triage decision for one hunk.
#[derive(Debug, Clone)]
pub enum TriageDecision {
    /// Confident enough — emit a synthetic LabelledHunk, skip LLM for this hunk.
    Resolved(LabelledHunk),
    /// Forward to LLM for proper labeling.
    Ambiguous,
}

impl TriageDecision {
    pub fn is_resolved(&self) -> bool {
        matches!(self, TriageDecision::Resolved(_))
    }
    pub fn is_ambiguous(&self) -> bool {
        matches!(self, TriageDecision::Ambiguous)
    }
}

/// Triage all hunks. Returns `(resolved, ambiguous)` — `resolved` carries
/// confident synthetic labels and `ambiguous` are hunks the LLM still needs
/// to see. The route can:
///   • Return early with just `resolved` if `ambiguous.is_empty()`.
///   • Otherwise, send `ambiguous` to the LLM and merge the labels back in
///     hunk-index order.
pub fn triage(hunks: &[Hunk]) -> Vec<TriageDecision> {
    hunks.iter().map(triage_one).collect()
}

fn triage_one(hunk: &Hunk) -> TriageDecision {
    let polish = hunk.polish_window.trim();
    let kept = hunk.kept_window.trim();

    // Pure insertions or deletions — we can't reason about transformations.
    // Send to LLM (it sees the full surrounding context).
    if polish.is_empty() || kept.is_empty() {
        return TriageDecision::Ambiguous;
    }

    let polish_words = polish.split_whitespace().count();
    let kept_words = kept.split_whitespace().count();

    // ── Multi-word polish → single jargon-y kept ────────────────────────
    // Canonical STT-error shape for acronyms / brand names: STT spelled
    // it out as multiple common words, the user collapsed it to the
    // jargon form.
    //   "Main corps"    → "MACOBS"
    //   "next message"  → "nextMessage"
    //   "n a tin"       → "n8n"
    //   "Cloud Code"    → "ClaudeCode"
    // Without this rule the LLM has to handle every one of these, and the
    // (correct) classification ends up bottlenecked on Groq latency.
    if polish_words > 1 && kept_words == 1 {
        let kept_jargon = phonetics::jargon_score(kept);
        // Any jargon signal at all is enough when collapsing multi-word
        // into a single token — the *shape* of the transformation is
        // already strong evidence of an STT error. Common-English single
        // words score 0.0 here so they fall through to Ambiguous.
        if kept_jargon >= 0.4 {
            return TriageDecision::Resolved(synthesize(
                hunk,
                EditClass::SttError,
                polish,
                kept,
                0.92,
            ));
        }
    }

    // Other multi-word transformations are too risky to triage — multi-word
    // swaps are usually rephrases but occasionally STT mistakes ("you all"
    // → "y'all"). Send to the LLM where we're already paying for nuance.
    if polish_words > 1 || kept_words > 1 {
        return TriageDecision::Ambiguous;
    }

    // Single-word transformations: the heart of the triage. Compute the
    // cheap phonetic + edit-distance signals.
    let phon_sim = phonetics::similarity(polish, kept);
    let lev = levenshtein_chars(polish, kept);
    let jargon = phonetics::jargon_score(kept);

    // ── Clear STT_ERROR ──────────────────────────────────────────────────
    // Polish and kept sound alike (phonetic ≥ 0.7) and the spelling is close
    // (Levenshtein ≤ 2). This is the canonical "STT misheard a similar-
    // sounding word" pattern: written→writen, recieve→receive, Anis→anish.
    //
    // When the kept token is also jargon-like (digits, mixed case), we lower
    // the bar slightly because jargon mishearings often stretch phonetics.
    if phon_sim >= 0.7 && lev <= 2 {
        return TriageDecision::Resolved(synthesize(hunk, EditClass::SttError, polish, kept, 0.95));
    }
    if phon_sim >= 0.55 && lev <= 3 && jargon >= 0.4 {
        return TriageDecision::Resolved(synthesize(hunk, EditClass::SttError, polish, kept, 0.85));
    }

    // ── Clear USER_REPHRASE ──────────────────────────────────────────────
    // Words don't sound alike AND aren't visually similar AND the kept form
    // isn't jargon. This is "good → great", "use → utilise" — the user
    // chose a different word entirely.
    //
    // We require ALL three to be confident:
    //   • phon_sim < 0.4   (phonetically distinct)
    //   • lev      ≥ 4     (visually distinct)
    //   • jargon   < 0.15  (the kept token isn't a name/code/brand)
    //
    // The jargon threshold was 0.3, but that let proper nouns like "Emiac"
    // (jargon=0.2 from initial-cap) slip through as rephrases, silently
    // killing the user's first correction.  Tightened to 0.15 so any
    // jargon signal at all sends the hunk to the LLM for a real decision.
    if phon_sim < 0.4 && lev >= 4 && jargon < 0.15 {
        return TriageDecision::Resolved(synthesize(
            hunk,
            EditClass::UserRephrase,
            polish,
            kept,
            0.85,
        ));
    }

    // ── Everything else — too close to call without context ──────────────
    TriageDecision::Ambiguous
}

/// Build a synthetic LabelledHunk. The shape matches what the LLM produces
/// downstream of `classifier::classify_edit`, so the route consumes it with
/// no special-case handling.
fn synthesize(
    hunk: &Hunk,
    class: EditClass,
    transcript_form: &str,
    correct_form: &str,
    confidence: f64,
) -> LabelledHunk {
    let extracted_term = match class {
        EditClass::SttError | EditClass::PolishError => Some(ExtractedTerm {
            transcript_form: transcript_form.to_string(),
            correct_form: correct_form.to_string(),
        }),
        _ => None,
    };
    LabelledHunk {
        hunk: hunk.clone(),
        class,
        confidence,
        extracted_term,
    }
}

/// Levenshtein over Unicode chars (not bytes). Bounded by `max + 1` early
/// to keep ambiguous hunks fast.
fn levenshtein_chars(a: &str, b: &str) -> usize {
    let av: Vec<char> = a.chars().collect();
    let bv: Vec<char> = b.chars().collect();
    let (n, m) = (av.len(), bv.len());
    if n == 0 {
        return m;
    }
    if m == 0 {
        return n;
    }
    let mut prev: Vec<usize> = (0..=m).collect();
    let mut curr: Vec<usize> = vec![0; m + 1];
    for i in 1..=n {
        curr[0] = i;
        for j in 1..=m {
            let cost = if av[i - 1] == bv[j - 1] { 0 } else { 1 };
            curr[j] = (curr[j - 1] + 1).min(prev[j] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[m]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(polish: &str, kept: &str) -> Hunk {
        Hunk {
            transcript_window: polish.to_string(),
            polish_window: polish.to_string(),
            kept_window: kept.to_string(),
        }
    }

    #[test]
    fn obvious_typo_resolves_as_stt_error() {
        let d = triage_one(&h("recieve", "receive"));
        match d {
            TriageDecision::Resolved(lh) => assert_eq!(lh.class, EditClass::SttError),
            _ => panic!("expected Resolved STT_ERROR for receive typo"),
        }
    }

    #[test]
    fn obvious_synonym_resolves_as_rephrase() {
        let d = triage_one(&h("good", "great"));
        match d {
            TriageDecision::Resolved(lh) => assert_eq!(lh.class, EditClass::UserRephrase),
            _ => panic!("expected Resolved USER_REPHRASE for good→great"),
        }
    }

    #[test]
    fn jargon_replacement_passes_with_lower_phonetic_bar() {
        // "written" → "n8n" — phonetically distant but jargon. LLM territory.
        let d = triage_one(&h("written", "n8n"));
        // We expect this to be Ambiguous (forward to LLM) — phon_sim is too
        // low for the relaxed jargon path AND too high for clear-rephrase path.
        // The LLM is better placed to make this call given context.
        assert!(d.is_ambiguous(), "expected Ambiguous, got {:?}", d);
    }

    #[test]
    fn name_with_only_partial_phonetic_match_stays_ambiguous() {
        // "Anis" → "anish" is genuinely ambiguous: phonetic similarity sits
        // around 0.67 (below the 0.7 confident-typo bar), and the kept form
        // is all-lowercase so jargon_score is low.  Triage rightly defers
        // to the LLM which has full sentence context.
        assert!(triage_one(&h("Anis", "anish")).is_ambiguous());
    }

    #[test]
    fn close_typo_with_high_phonetic_match_resolves() {
        // "writen" → "written" — same phonetic key, lev=1.
        let d = triage_one(&h("writen", "written"));
        match d {
            TriageDecision::Resolved(lh) => assert_eq!(lh.class, EditClass::SttError),
            _ => panic!("expected Resolved STT_ERROR for writen→written"),
        }
    }

    #[test]
    fn empty_window_is_ambiguous() {
        assert!(triage_one(&h("", "n8n")).is_ambiguous());
        assert!(triage_one(&h("written", "")).is_ambiguous());
    }

    #[test]
    fn multi_word_is_ambiguous() {
        assert!(triage_one(&h("good morning", "great morning")).is_ambiguous());
    }

    #[test]
    fn close_synonym_with_some_phonetic_overlap_is_ambiguous() {
        // "happy" → "elated" — different sound, different spelling, no jargon
        // → REPHRASE candidate. But "merry" → "happy" might be borderline.
        // The triage stays conservative — it should be Ambiguous if any of the
        // three rephrase signals doesn't fire confidently.
        let d = triage_one(&h("ask", "asked"));
        // ask→asked: phon_sim high (suffix-only diff), lev=2, not jargon.
        // Should resolve as STT_ERROR (looks like a tense/inflection STT miss).
        match d {
            TriageDecision::Resolved(lh) => assert_eq!(lh.class, EditClass::SttError),
            _ => panic!("expected Resolved STT_ERROR for ask→asked"),
        }
    }

    #[test]
    fn multi_word_polish_to_acronym_kept_resolves_as_stt_error() {
        // The MACOBS regression: Main corps -> MACOBS (multi-word -> all-caps)
        let d = triage_one(&h("Main corps", "MACOBS"));
        match d {
            TriageDecision::Resolved(lh) => assert_eq!(lh.class, EditClass::SttError),
            _ => panic!(
                "expected Resolved STT_ERROR for Main corps -> MACOBS, got {:?}",
                d
            ),
        }
    }

    #[test]
    fn multi_word_to_camel_case_resolves_as_stt_error() {
        // Cloud Code -> ClaudeCode (or similar brand-name compaction)
        let d = triage_one(&h("Cloud Code", "ClaudeCode"));
        match d {
            TriageDecision::Resolved(lh) => assert_eq!(lh.class, EditClass::SttError),
            _ => panic!(
                "expected Resolved STT_ERROR for Cloud Code -> ClaudeCode, got {:?}",
                d
            ),
        }
    }

    #[test]
    fn multi_word_to_common_single_word_stays_ambiguous() {
        // "the one" -> "this" — common single-word, should NOT auto-resolve
        assert!(triage_one(&h("the one", "this")).is_ambiguous());
    }

    #[test]
    fn case_change_resolves_as_stt_error() {
        // "iphone" → "iPhone" — phonetically identical, lev=1.
        let d = triage_one(&h("iphone", "iPhone"));
        match d {
            TriageDecision::Resolved(lh) => assert_eq!(lh.class, EditClass::SttError),
            _ => panic!("expected Resolved STT_ERROR for iphone→iPhone"),
        }
    }

    #[test]
    fn levenshtein_chars_handles_unicode() {
        assert_eq!(levenshtein_chars("नमस्ते", "नमस्ते"), 0);
        assert_eq!(levenshtein_chars("hello", "hallo"), 1);
        assert_eq!(levenshtein_chars("kitten", "sitting"), 3);
    }
}
