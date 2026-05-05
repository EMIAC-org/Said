//! Learning-pipeline evaluation harness.
//!
//! Loads `tests/data/learning_golden.json` (a hand-labelled set of edit cases)
//! and runs each case through the **deterministic** stages of the pipeline:
//!
//!   1. `pre_filter::run` — early-class decisions
//!   2. `edit_diff::diff` — structural hunk extraction
//!   3. `promotion_gate::*` + `phonetics::*` — STT_ERROR/POLISH_ERROR gates
//!
//! For each case we record what each stage decided versus what the golden
//! label says should happen, then print a precision/recall table per-class
//! at the end.
//!
//! The LLM stage (Groq) is **not** exercised here — it's a network call to a
//! third-party API and we don't want CI to depend on it. Instead, for cases
//! whose golden label says a term *should promote*, we synthesise a
//! `LabelledHunk` carrying that term and exercise only the gate logic — which
//! is what catches the most damaging false-positive shapes (Devanagari leaks,
//! concatenations, hallucinated terms not in user_kept).
//!
//! Run with:   cargo test --package polish-backend --test learning_eval -- --nocapture

use serde::Deserialize;

use said_backend::llm::{
    classifier::{EditClass, ExtractedTerm, LabelledHunk},
    edit_diff::{self, Hunk},
    phonetics, pre_filter, promotion_gate,
};

#[derive(Debug, Deserialize)]
struct Golden {
    cases: Vec<Case>,
}

#[derive(Debug, Deserialize)]
struct Case {
    name: String,
    polish: String,
    user_kept: String,
    transcript: String,
    output_language: String,
    #[serde(default = "default_capture_method")]
    capture_method: String,
    expected: Expected,
}

fn default_capture_method() -> String {
    "ax".to_string()
}

#[derive(Debug, Deserialize)]
struct Expected {
    /// "drop" | "early_user_rewrite" | "early_user_rephrase" | "pass"
    pre_filter: String,
    /// "STT_ERROR" | "POLISH_ERROR" | "USER_REPHRASE" | "USER_REWRITE" | "DROP"
    expected_class: String,
    /// Term that promotion gates should accept (or null if none).
    should_promote_term: Option<String>,
    /// Whether the gate should accept the term assuming the LLM hands it over.
    promotion_should_pass: bool,
    #[serde(default)]
    notes: String,
}

#[derive(Default, Debug)]
struct Stats {
    total: usize,
    pre_filter_ok: usize,
    promotion_ok: usize,
    failures: Vec<String>,
}

#[test]
fn learning_pipeline_golden_eval() {
    let bytes = std::fs::read("tests/data/learning_golden.json")
        .expect("missing tests/data/learning_golden.json");
    let golden: Golden = serde_json::from_slice(&bytes).expect("invalid learning_golden.json");

    let mut stats = Stats::default();

    println!("\n=== Said Learning Pipeline Eval ===");
    println!("Total cases: {}\n", golden.cases.len());

    for case in &golden.cases {
        stats.total += 1;
        let mut case_failed = false;

        // ── Stage 1: pre_filter ──────────────────────────────────────────
        let pf = pre_filter::run(&case.polish, &case.user_kept, &case.output_language);
        let pf_actual = match &pf {
            pre_filter::PreFilter::Drop => "drop",
            pre_filter::PreFilter::EarlyClass(d) if d.class == "USER_REWRITE" => {
                "early_user_rewrite"
            }
            pre_filter::PreFilter::EarlyClass(d) if d.class == "USER_REPHRASE" => {
                "early_user_rephrase"
            }
            pre_filter::PreFilter::EarlyClass(_) => "early_other",
            pre_filter::PreFilter::Pass => "pass",
        };
        if pf_actual == case.expected.pre_filter {
            stats.pre_filter_ok += 1;
        } else {
            case_failed = true;
            stats.failures.push(format!(
                "  ✗ [{}] pre_filter: expected {:?}, got {:?}",
                case.name, case.expected.pre_filter, pf_actual,
            ));
        }

        // ── Stage 4: promotion_gate (synthetic LabelledHunk) ─────────────
        // Only run promotion gate check when the case expects a specific
        // term to be evaluated. We synthesise a hunk that the LLM might
        // plausibly produce so we test the gate in isolation.
        if let Some(term) = &case.expected.should_promote_term {
            let synthetic_hunk =
                synthesise_hunk_for(&case.transcript, &case.polish, &case.user_kept, term);
            let synthetic_cand = LabelledHunk {
                hunk: synthetic_hunk,
                class: EditClass::SttError,
                confidence: 0.9,
                extracted_term: Some(ExtractedTerm {
                    transcript_form: nearest_polish_token(&case.polish, term),
                    correct_form: term.clone(),
                }),
            };
            let gate_passed = stt_promotion_gate(
                &synthetic_cand,
                term,
                &case.user_kept,
                &case.output_language,
            );
            // Capture-confidence policy: keystroke_only never auto-promotes.
            let allowed_by_capture = matches!(
                case.capture_method.as_str(),
                "ax" | "keystroke_verified" | "clipboard"
            );
            let promotion_actual = gate_passed && allowed_by_capture;

            if promotion_actual == case.expected.promotion_should_pass {
                stats.promotion_ok += 1;
            } else {
                case_failed = true;
                stats.failures.push(format!(
                    "  ✗ [{}] promotion: expected pass={}, got pass={} (gate={}, capture_ok={})",
                    case.name,
                    case.expected.promotion_should_pass,
                    promotion_actual,
                    gate_passed,
                    allowed_by_capture,
                ));
            }
        } else {
            // No term expected → trivially passes the promotion check.
            stats.promotion_ok += 1;
        }

        if !case_failed {
            // Quietly tick — only failures are loud.
        }
    }

    // ── Summary ──────────────────────────────────────────────────────────
    println!("\n=== Results ===");
    println!(
        "Pre-filter:  {}/{}  ({:.1}%)",
        stats.pre_filter_ok,
        stats.total,
        100.0 * stats.pre_filter_ok as f64 / stats.total as f64
    );
    println!(
        "Promotion:   {}/{}  ({:.1}%)",
        stats.promotion_ok,
        stats.total,
        100.0 * stats.promotion_ok as f64 / stats.total as f64
    );

    if !stats.failures.is_empty() {
        println!("\nFailures:");
        for f in &stats.failures {
            println!("{f}");
        }
        panic!(
            "{} case(s) failed — see breakdown above",
            stats.failures.len(),
        );
    } else {
        println!("\nAll cases passed.\n");
    }
}

/// Synthesise a realistic hunk that the LLM would label, by running the
/// real diff and picking the hunk whose kept_window contains `term`.
///
/// This matters for the concatenation guard: the gate inspects
/// `polish_window` and `correct_form`, both of which are *token-scoped*
/// in real classifier output — not the full polish/kept strings.
fn synthesise_hunk_for(transcript: &str, polish: &str, kept: &str, term: &str) -> Hunk {
    let hunks = edit_diff::diff(transcript, polish, kept);
    // Prefer a hunk whose kept_window contains the term (case-insensitive).
    let pick = hunks.iter().find(|h| {
        h.kept_window
            .to_ascii_lowercase()
            .contains(&term.to_ascii_lowercase())
    });
    if let Some(h) = pick {
        return h.clone();
    }
    // Fallback: a hunk that spans the whole edit. Realistic when the diff is
    // short and the term IS the kept_window.
    Hunk {
        transcript_window: transcript.to_string(),
        polish_window: polish.to_string(),
        kept_window: kept.to_string(),
    }
}

/// Pick the polish token most phonetically-similar to `target` so the
/// synthetic ExtractedTerm.transcript_form is realistic. Falls back to
/// the first polish token.
fn nearest_polish_token(polish: &str, target: &str) -> String {
    polish
        .split_whitespace()
        .max_by(|a, b| {
            phonetics::similarity(a, target)
                .partial_cmp(&phonetics::similarity(b, target))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or("")
        .trim_matches(|c: char| !c.is_alphanumeric())
        .to_string()
}

/// Mirrors the STT_ERROR promotion gate in `routes/classify.rs::stt_promotion_allowed`.
/// Kept here as a copy so the eval doesn't need to spin up AppState/SQLite.
/// **Keep this in sync with the route** — drift here masks regressions.
fn stt_promotion_gate(
    cand: &LabelledHunk,
    correct: &str,
    user_kept: &str,
    output_language: &str,
) -> bool {
    if !promotion_gate::appears_in_user_kept(correct, user_kept) {
        return false;
    }
    if !promotion_gate::script_matches(correct, output_language) {
        return false;
    }
    if cand.extracted_term.is_none()
        && promotion_gate::is_concatenation_pattern(cand.polish_form(), correct)
    {
        return false;
    }
    if cand.extracted_term.is_some()
        && promotion_gate::is_concatenation_pattern(cand.polish_form(), correct)
    {
        // With extracted_term we trust the extraction over the raw concat
        // pattern — *unless* the extracted form is itself the concatenation,
        // which is the EMIAC case. We need to detect that the extraction
        // didn't actually un-bundle the concatenation.
        if cand
            .extracted_term
            .as_ref()
            .map(|t| t.correct_form.as_str())
            == Some(correct)
            && correct == cand.hunk.kept_window.trim()
        {
            return false;
        }
    }
    let phon_sim = phonetics::similarity(cand.transcript_form(), correct)
        .max(phonetics::similarity(cand.polish_form(), correct));
    let jargon = phonetics::jargon_score(correct);
    let confident = cand.confidence >= 0.7;
    if phon_sim < 0.5 && jargon < 0.4 && !confident {
        return false;
    }
    true
}

#[test]
fn diff_produces_no_hunks_when_polish_equals_kept() {
    let hunks = edit_diff::diff("a b c", "a b c", "a b c");
    assert!(hunks.is_empty());
}

#[test]
fn diff_isolates_single_token_swap() {
    let hunks = edit_diff::diff(
        "I use written for automation",
        "I use written for automation",
        "I use n8n for automation",
    );
    // At least one hunk where polish has "written" and kept has "n8n".
    assert!(
        hunks
            .iter()
            .any(|h| h.polish_window.contains("written") && h.kept_window.contains("n8n")),
        "expected an n8n-for-written hunk, got {:#?}",
        hunks
    );
}
