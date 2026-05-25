//! Eval harness for the Said learning pipeline.
//!
//! Three test layers:
//!   1. RETRIEVAL — seed vocab, sweep 14K transcripts, assert 0 false injections
//!   2. STORAGE   — simulate edits, verify correct diffs/gates/promotions
//!   3. POLLUTION — store a correction, sweep 14K transcripts, verify no leakage
//!
//! Usage:
//!   cargo run -p said-backend --bin eval-pipeline -- \
//!       --transcripts tools/eval-pipeline/transcripts.jsonl \
//!       --vocab tools/eval-pipeline/vocab_seed.json \
//!       --dev-quality tools/eval-pipeline/dev_terms_quality.json

use clap::Parser;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

use said_backend::llm::vocab_resolver;
use said_backend::store::corrections::{self, Correction};
use said_backend::store::pending_promotions;
use said_backend::store::stt_replacements::ApplyResult;
use said_backend::store::tier2_edit_policy;
use said_backend::store::vocab_embeddings;
use said_backend::store::vocab_fts;
use said_backend::store::vocabulary::{self, VocabTerm};
use said_backend::store::{self, DbPool};
use said_backend::tier2;

#[derive(Parser)]
struct Args {
    #[arg(long)]
    transcripts: PathBuf,
    #[arg(long)]
    vocab: PathBuf,
    #[arg(long)]
    dev_quality: Option<PathBuf>,
}

#[derive(Deserialize)]
struct TranscriptRow {
    #[allow(dead_code)]
    lang: String,
    text: String,
}

#[derive(Deserialize)]
struct VocabSeed {
    term: String,
    #[serde(rename = "type")]
    term_type: String,
    meaning: String,
    context: String,
    source: String,
}

#[derive(Deserialize)]
struct DevQualitySuite {
    terms: Vec<DevQualityTerm>,
    #[serde(default)]
    negatives: Vec<DevQualityNegative>,
}

#[derive(Deserialize)]
struct DevQualityTerm {
    term: String,
    #[serde(rename = "type")]
    term_type: String,
    meaning: String,
    context: String,
    #[serde(default = "default_dev_quality_source")]
    source: String,
    positive_template: String,
    distortions: Vec<String>,
}

#[derive(Deserialize)]
struct DevQualityNegative {
    text: String,
    forbidden_terms: Vec<String>,
}

fn main() {
    let args = Args::parse();
    let transcripts = load_transcripts(&args.transcripts);
    let seeds = load_seeds(&args.vocab);
    let dev_quality = args
        .dev_quality
        .as_ref()
        .map(|path| load_dev_quality_suite(path));
    eprintln!(
        "Loaded {} transcripts, {} vocab seeds\n",
        transcripts.len(),
        seeds.len()
    );

    let mut total_pass = 0;
    let mut total_fail = 0;
    let mut failures: Vec<String> = Vec::new();

    // ═══════════════════════════════════════════════════════════
    //  LAYER 1: RETRIEVAL — vocab terms must not false-inject
    // ═══════════════════════════════════════════════════════════
    println!("══ LAYER 1: RETRIEVAL (vocab → 14K transcript sweep) ══\n");
    let pool = setup_db(&seeds);
    let user_id = "eval-user";
    let all_terms = vocabulary::top_terms(&pool, user_id, 1000);

    let fi = sweep_retrieval(&pool, user_id, &all_terms, &transcripts);
    check(
        &mut total_pass,
        &mut total_fail,
        &mut failures,
        "Vocab false injections = 0",
        fi.false_count == 0,
        &format!(
            "{} false injections (first: {})",
            fi.false_count, fi.first_detail
        ),
    );

    // Test corrections retrieval
    let test_corrections = vec![
        Correction {
            wrong: "badhiya".into(),
            right: "badiya".into(),
            count: 3,
        },
        Correction {
            wrong: "there".into(),
            right: "their".into(),
            count: 5,
        },
        Correction {
            wrong: "recieve".into(),
            right: "receive".into(),
            count: 2,
        },
    ];
    let cf = sweep_corrections(&test_corrections, &transcripts);
    check(
        &mut total_pass,
        &mut total_fail,
        &mut failures,
        "Correction false injections = 0",
        cf.false_count == 0,
        &format!(
            "{} false injections (first: {})",
            cf.false_count, cf.first_detail
        ),
    );

    // ═══════════════════════════════════════════════════════════
    //  LAYER 2: STORAGE — extract_diffs + promotion gates
    // ═══════════════════════════════════════════════════════════
    println!("\n══ LAYER 2: STORAGE (edit simulation → verify diffs/gates) ══\n");

    // 2a: extract_diffs — same word count, positional alignment
    let diffs = corrections::extract_diffs("main course ka IPO", "main MACOBS ka IPO");
    check(
        &mut total_pass,
        &mut total_fail,
        &mut failures,
        "extract_diffs: 'course'→'macobs' (same word count)",
        diffs.len() == 1 && diffs[0].0 == "course" && diffs[0].1 == "macobs",
        &format!("got {:?}", diffs),
    );

    // Different word count → empty (by design)
    let diffs = corrections::extract_diffs("main course ka IPO", "MACOBS ka IPO");
    check(
        &mut total_pass,
        &mut total_fail,
        &mut failures,
        "extract_diffs: word count mismatch → empty",
        diffs.is_empty(),
        &format!("got {:?}", diffs),
    );

    // Identical → no diffs
    let diffs = corrections::extract_diffs("hello world", "hello world");
    check(
        &mut total_pass,
        &mut total_fail,
        &mut failures,
        "extract_diffs: identical → empty",
        diffs.is_empty(),
        &format!("got {:?}", diffs),
    );

    // Case change — extract_diffs lowercases both sides, so same case = no diff
    let diffs = corrections::extract_diffs("macobs ka IPO", "MACOBS ka IPO");
    check(
        &mut total_pass,
        &mut total_fail,
        &mut failures,
        "extract_diffs: case-only change → empty (case-insensitive)",
        diffs.is_empty(),
        &format!("got {:?}", diffs),
    );

    // Actual word swap
    let diffs = corrections::extract_diffs("I recieve the mail", "I receive the mail");
    check(
        &mut total_pass,
        &mut total_fail,
        &mut failures,
        "extract_diffs: 'recieve'→'receive'",
        diffs.len() == 1 && diffs[0].0 == "recieve" && diffs[0].1 == "receive",
        &format!("got {:?}", diffs),
    );

    // Punctuation stripped
    let diffs = corrections::extract_diffs("hello, world.", "hello world");
    check(
        &mut total_pass,
        &mut total_fail,
        &mut failures,
        "extract_diffs: punctuation stripped → no diff",
        diffs.is_empty(),
        &format!("got {:?}", diffs),
    );

    // 2b: K-threshold = 3 (needs 3 sightings to promote)
    let promo_pool = setup_empty_db_with_user("u1");
    let d1 = pending_promotions::record_sighting(
        &promo_pool,
        "u1",
        "MACOBS",
        "main course",
        "hinglish",
        pending_promotions::DEFAULT_K,
    );
    check(
        &mut total_pass,
        &mut total_fail,
        &mut failures,
        "k-threshold: sighting 1 → Pending",
        matches!(
            d1,
            Some(pending_promotions::PromotionDecision::Pending { .. })
        ),
        &format!("got {:?}", d1),
    );

    let d2 = pending_promotions::record_sighting(
        &promo_pool,
        "u1",
        "MACOBS",
        "main course",
        "hinglish",
        pending_promotions::DEFAULT_K,
    );
    check(
        &mut total_pass,
        &mut total_fail,
        &mut failures,
        "k-threshold: sighting 2 → still Pending (k=3)",
        matches!(
            d2,
            Some(pending_promotions::PromotionDecision::Pending { .. })
        ),
        &format!("got {:?}", d2),
    );

    let d3 = pending_promotions::record_sighting(
        &promo_pool,
        "u1",
        "MACOBS",
        "main course",
        "hinglish",
        pending_promotions::DEFAULT_K,
    );
    check(
        &mut total_pass,
        &mut total_fail,
        &mut failures,
        "k-threshold: sighting 3 → Promote",
        matches!(
            d3,
            Some(pending_promotions::PromotionDecision::Promote { .. })
        ),
        &format!("got {:?}", d3),
    );

    // 2c: Temporal decay — stale sighting resets count
    let decay_pool = setup_empty_db_with_user("u1");
    // Insert a sighting with old timestamp manually
    {
        let conn = decay_pool.get().unwrap();
        let old_ts = said_backend::store::now_ms() - (15 * 24 * 60 * 60 * 1000); // 15 days ago
        conn.execute(
            "INSERT INTO pending_promotions
               (user_id, correct_form, transcript_form, phonetic_key,
                output_language, sighting_count, first_seen, last_seen)
             VALUES ('u1','STALE','stale','STL','hinglish',2,?1,?1)",
            rusqlite::params![old_ts],
        )
        .unwrap();
    }
    let d_stale = pending_promotions::record_sighting(
        &decay_pool,
        "u1",
        "STALE",
        "stale",
        "hinglish",
        pending_promotions::DEFAULT_K,
    );
    check(
        &mut total_pass,
        &mut total_fail,
        &mut failures,
        "temporal decay: stale sighting (15d) resets → Pending(1)",
        matches!(
            d_stale,
            Some(pending_promotions::PromotionDecision::Pending { sighting_count: 1 })
        ),
        &format!("got {:?}", d_stale),
    );

    // 2d: Demotion — weight drops by 1.0 per removal, deleted at 0
    let demo_pool = setup_empty_db_with_user("u1");
    vocabulary::upsert(&demo_pool, "u1", "BadTerm", 3.0, "auto");
    for i in 1..=3 {
        vocabulary::demote(&demo_pool, "u1", "BadTerm", 1.0);
        let remaining = vocabulary::top_terms(&demo_pool, "u1", 100);
        let exists = remaining.iter().any(|t| t.term == "BadTerm");
        if i < 3 {
            check(
                &mut total_pass,
                &mut total_fail,
                &mut failures,
                &format!("demotion: after {i} removals → still exists"),
                exists,
                &format!("exists={exists}"),
            );
        } else {
            check(
                &mut total_pass,
                &mut total_fail,
                &mut failures,
                "demotion: after 3 removals (weight 0) → deleted",
                !exists,
                &format!("exists={exists}"),
            );
        }
    }

    // ═══════════════════════════════════════════════════════════
    //  LAYER 3: POLLUTION — store correction, sweep 14K
    // ═══════════════════════════════════════════════════════════
    println!("\n══ LAYER 3: POLLUTION (store → sweep → verify no leakage) ══\n");

    // 3a: Store "there→their", sweep 14K — only transcripts with literal "there" should match
    let there_corr = vec![Correction {
        wrong: "there".into(),
        right: "their".into(),
        count: 5,
    }];
    let pollution = sweep_corrections(&there_corr, &transcripts);
    check(
        &mut total_pass,
        &mut total_fail,
        &mut failures,
        "Pollution: 'there→their' on 14K transcripts",
        pollution.false_count == 0,
        &format!("{} false injections", pollution.false_count),
    );

    // 3b: Store common-word vocab terms, sweep 14K
    let common_seeds = vec![
        VocabSeed {
            term: "time".into(),
            term_type: "other".into(),
            meaning: "Duration".into(),
            context: "time tracking sprint".into(),
            source: "auto".into(),
        },
        VocabSeed {
            term: "can".into(),
            term_type: "other".into(),
            meaning: "Modal verb".into(),
            context: "can we fix this".into(),
            source: "auto".into(),
        },
        VocabSeed {
            term: "go".into(),
            term_type: "other".into(),
            meaning: "Verb".into(),
            context: "go run tests".into(),
            source: "auto".into(),
        },
    ];
    let common_pool = setup_db(&common_seeds);
    let common_terms = vocabulary::top_terms(&common_pool, user_id, 1000);
    let common_fi = sweep_retrieval(&common_pool, user_id, &common_terms, &transcripts);
    check(
        &mut total_pass,
        &mut total_fail,
        &mut failures,
        "Pollution: common words (time/can/go) on 14K",
        common_fi.false_count == 0,
        &format!(
            "{} false injections (first: {})",
            common_fi.false_count, common_fi.first_detail
        ),
    );

    // 3c: Adversarial — short terms that might substring-match
    let adversarial_cases = vec![
        ("8GB", "128 GB RAM hai mere laptop mein"),
        ("8GB", "8 ghante baad aana"),
        ("RT", "return the value please"),
        ("RAM", "ramadan mubarak bhai"),
        ("API", "capital city of India"),
        ("can", "cancer treatment is expensive"),
        ("go", "google search karo"),
        ("PR", "prayer time ho gaya"),
        ("SQL", "sequel to the movie"),
        ("EMI", "emission control system"),
    ];
    for (term, transcript) in &adversarial_cases {
        let adv_seeds = vec![VocabSeed {
            term: term.to_string(),
            term_type: if term.len() <= 3 {
                "acronym".into()
            } else {
                "other".into()
            },
            meaning: "Test term".into(),
            context: format!("{term} test context"),
            source: "auto".into(),
        }];
        let adv_pool = setup_db(&adv_seeds);
        let adv_terms = vocabulary::top_terms(&adv_pool, user_id, 100);
        let alias_result = ApplyResult {
            text: transcript.to_string(),
            matches: vec![],
            traces: vec![],
        };
        let selected = vocab_embeddings::select_for_prompt(
            &adv_pool,
            user_id,
            "hinglish",
            None,
            Some(transcript),
        );
        let resolved =
            vocab_resolver::resolve_for_prompt(transcript, &selected, &adv_terms, &alias_result);
        let injected = !resolved.resolved_terms.is_empty();
        check(
            &mut total_pass,
            &mut total_fail,
            &mut failures,
            &format!(
                "Adversarial: '{term}' must NOT inject into '{}'",
                truncate(transcript, 40)
            ),
            !injected,
            &format!(
                "injected={injected} resolved={:?}",
                resolved
                    .resolved_terms
                    .iter()
                    .map(|t| &t.term)
                    .collect::<Vec<_>>()
            ),
        );
    }

    // 3d: Positive cases — terms SHOULD inject when present
    let positive_cases = vec![
        ("MACOBS", "MACOBS ka stock price kya hai"),
        ("Anish", "Anish ko call karo please"),
        ("localhost", "localhost pe server start karo"),
        ("kubectl", "kubectl apply karo deployment"),
    ];
    for (term, transcript) in &positive_cases {
        let pos_seeds = vec![VocabSeed {
            term: term.to_string(),
            term_type: "proper_noun".into(),
            meaning: "Test".into(),
            context: format!("{term} is important"),
            source: "auto".into(),
        }];
        let pos_pool = setup_db(&pos_seeds);
        let pos_terms = vocabulary::top_terms(&pos_pool, user_id, 100);
        let alias_result = ApplyResult {
            text: transcript.to_string(),
            matches: vec![],
            traces: vec![],
        };
        let selected = vocab_embeddings::select_for_prompt(
            &pos_pool,
            user_id,
            "hinglish",
            None,
            Some(transcript),
        );
        let resolved =
            vocab_resolver::resolve_for_prompt(transcript, &selected, &pos_terms, &alias_result);
        let injected = resolved.resolved_terms.iter().any(|t| t.term == *term);
        check(
            &mut total_pass,
            &mut total_fail,
            &mut failures,
            &format!(
                "Positive: '{term}' SHOULD inject into '{}'",
                truncate(transcript, 40)
            ),
            injected,
            &format!(
                "injected={injected} resolved={:?}",
                resolved
                    .resolved_terms
                    .iter()
                    .map(|t| &t.term)
                    .collect::<Vec<_>>()
            ),
        );
    }

    // ═══════════════════════════════════════════════════════════
    //  LAYER 4: DEV TERM QUALITY — learned variants + no pollution
    // ═══════════════════════════════════════════════════════════
    if let Some(dev_quality) = dev_quality.as_ref() {
        println!("\n══ LAYER 4: DEV TERM QUALITY (distortions → learned policy) ══\n");
        run_dev_quality_suite(dev_quality, &mut total_pass, &mut total_fail, &mut failures);
    }

    // ═══════════════════════════════════════════════════════════
    //  SUMMARY
    // ═══════════════════════════════════════════════════════════
    println!("\n{}", "=".repeat(60));
    println!("  RESULTS: {} passed, {} failed", total_pass, total_fail);
    println!("{}", "=".repeat(60));

    if !failures.is_empty() {
        println!("\nFAILURES:\n");
        for f in &failures {
            println!("  FAIL: {f}");
        }
    }

    println!("\n{}", if total_fail == 0 { "PASS" } else { "FAIL" });
    if total_fail > 0 {
        std::process::exit(1);
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn check(
    pass: &mut usize,
    fail: &mut usize,
    failures: &mut Vec<String>,
    name: &str,
    ok: bool,
    detail: &str,
) {
    if ok {
        println!("  PASS  {name}");
        *pass += 1;
    } else {
        println!("  FAIL  {name}");
        println!("        {detail}");
        *fail += 1;
        failures.push(format!("{name}: {detail}"));
    }
}

struct SweepResult {
    false_count: usize,
    first_detail: String,
}

fn sweep_retrieval(
    pool: &DbPool,
    user_id: &str,
    all_terms: &[VocabTerm],
    transcripts: &[TranscriptRow],
) -> SweepResult {
    let mut false_count = 0;
    let mut first_detail = String::new();
    for (i, row) in transcripts.iter().enumerate() {
        if i > 0 && i % 5000 == 0 {
            eprint!("  ... {i}/{} ", transcripts.len());
        }
        let transcript = row.text.trim();
        if transcript.len() < 5 {
            continue;
        }

        let selected =
            vocab_embeddings::select_for_prompt(pool, user_id, "hinglish", None, Some(transcript));
        let alias_result = ApplyResult {
            text: transcript.to_string(),
            matches: vec![],
            traces: vec![],
        };
        let resolved =
            vocab_resolver::resolve_for_prompt(transcript, &selected, all_terms, &alias_result);
        for rt in &resolved.resolved_terms {
            if is_false_injection(transcript, &rt.term) {
                false_count += 1;
                if first_detail.is_empty() {
                    first_detail = format!("'{}' in '{}'", rt.term, truncate(transcript, 60));
                }
            }
        }
    }
    if transcripts.len() > 5000 {
        eprintln!();
    }
    SweepResult {
        false_count,
        first_detail,
    }
}

fn sweep_corrections(corrections: &[Correction], transcripts: &[TranscriptRow]) -> SweepResult {
    let mut false_count = 0;
    let mut first_detail = String::new();
    for row in transcripts {
        let transcript = row.text.trim();
        if transcript.len() < 5 {
            continue;
        }
        let relevant = corrections::filter_relevant(corrections, transcript, 2, 10);
        for c in &relevant {
            let wrong_lower = c.wrong.to_ascii_lowercase();
            let tl = transcript.to_ascii_lowercase();
            let tokens: Vec<&str> = tl
                .split(|c: char| !c.is_alphanumeric())
                .filter(|s| !s.is_empty())
                .collect();
            if !tokens.iter().any(|t| *t == wrong_lower) {
                false_count += 1;
                if first_detail.is_empty() {
                    first_detail = format!(
                        "'{}→{}' in '{}'",
                        c.wrong,
                        c.right,
                        truncate(transcript, 60)
                    );
                }
            }
        }
    }
    SweepResult {
        false_count,
        first_detail,
    }
}

fn is_false_injection(transcript: &str, term: &str) -> bool {
    let tl = transcript.to_ascii_lowercase();
    let term_l = term.to_ascii_lowercase();
    let tokens: Vec<&str> = tl
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect();
    let term_tokens: Vec<&str> = term_l
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect();
    if term_tokens.len() == 1 {
        !tokens.iter().any(|t| *t == term_tokens[0])
    } else {
        !tokens
            .windows(term_tokens.len())
            .any(|w| w.iter().zip(term_tokens.iter()).all(|(a, b)| a == b))
    }
}

fn run_dev_quality_suite(
    suite: &DevQualitySuite,
    total_pass: &mut usize,
    total_fail: &mut usize,
    failures: &mut Vec<String>,
) {
    let user_id = "eval-user";
    let seeds: Vec<VocabSeed> = suite
        .terms
        .iter()
        .map(|term| VocabSeed {
            term: term.term.clone(),
            term_type: term.term_type.clone(),
            meaning: term.meaning.clone(),
            context: term.context.clone(),
            source: term.source.clone(),
        })
        .collect();
    let total_distortions: usize = suite.terms.iter().map(|term| term.distortions.len()).sum();
    let duplicate_variants = duplicate_distortions(suite);
    check(
        total_pass,
        total_fail,
        failures,
        "Dev quality fixture: every term has at least 6 distortions and no duplicate variant conflicts",
        suite.terms.iter().all(|term| term.distortions.len() >= 6) && duplicate_variants.is_empty(),
        &format!(
            "terms={} distortions={} duplicate_conflicts={:?}",
            suite.terms.len(),
            total_distortions,
            duplicate_variants
        ),
    );

    let shadow_pool = setup_db(&seeds);
    let shadow_terms = vocabulary::top_terms(&shadow_pool, user_id, 1000);
    let mut shadow_mutations = 0usize;
    let mut shadow_correct = 0usize;
    let mut first_shadow_miss = String::new();

    for term in &suite.terms {
        for variant in &term.distortions {
            let transcript = render_dev_quality_transcript(&term.positive_template, variant);
            let result =
                tier2::correct_with_store(&shadow_pool, user_id, &transcript, &[], &shadow_terms);
            if result.text != transcript {
                shadow_mutations += 1;
            }
            if trace_points_to_term(&result, variant, &term.term) {
                shadow_correct += 1;
            } else if first_shadow_miss.is_empty() {
                first_shadow_miss = format!(
                    "{} → {} in '{}'",
                    variant,
                    term.term,
                    truncate(&transcript, 80)
                );
            }
        }
    }
    check(
        total_pass,
        total_fail,
        failures,
        "Dev quality shadow: fuzzy scorer never mutates transcript",
        shadow_mutations == 0,
        &format!("{shadow_mutations}/{total_distortions} shadow transcripts mutated"),
    );
    let shadow_coverage = if total_distortions == 0 {
        0.0
    } else {
        shadow_correct as f64 / total_distortions as f64
    };
    check(
        total_pass,
        total_fail,
        failures,
        "Dev quality shadow: scorer points to expected term in >=70% of variants",
        shadow_coverage >= 0.70,
        &format!(
            "{shadow_correct}/{total_distortions} ({:.1}%), first miss: {}",
            shadow_coverage * 100.0,
            first_shadow_miss
        ),
    );

    let learned_pool = setup_db(&seeds);
    let mut failed_rule_writes = 0usize;
    let mut rule_failures = Vec::new();
    for term in &suite.terms {
        let (left_context, right_context) = template_context(&term.positive_template);
        for variant in &term.distortions {
            let first = tier2_edit_policy::record_explicit_edit(
                &learned_pool,
                user_id,
                variant,
                &term.term,
                "replace",
                &left_context,
                &right_context,
                None,
            );
            let second = tier2_edit_policy::record_explicit_edit(
                &learned_pool,
                user_id,
                variant,
                &term.term,
                "replace",
                &left_context,
                &right_context,
                None,
            );
            if !(first && second) {
                failed_rule_writes += 1;
                if rule_failures.len() < 10 {
                    rule_failures.push(format!("{variant} -> {}", term.term));
                }
            }
        }
    }
    let status = tier2_edit_policy::status(&learned_pool, user_id);
    check(
        total_pass,
        total_fail,
        failures,
        "Dev quality learning: 2 confirmations activate every variant rule",
        failed_rule_writes == 0 && status.active_rule_count as usize == total_distortions,
        &format!(
            "failed_writes={failed_rule_writes}, active_rules={}, expected={}, first={}",
            status.active_rule_count,
            total_distortions,
            rule_failures.join(", ")
        ),
    );

    let learned_terms = vocabulary::top_terms(&learned_pool, user_id, 1000);
    let mut active_failures = 0usize;
    let mut first_active_failure = String::new();
    for term in &suite.terms {
        for variant in &term.distortions {
            let transcript = render_dev_quality_transcript(&term.positive_template, variant);
            let result =
                tier2::correct_with_store(&learned_pool, user_id, &transcript, &[], &learned_terms);
            let corrected = contains_termish(&result.text, &term.term);
            let removed_variant = !contains_token_norm(&result.text, variant);
            let edit_policy_match = result.matches.iter().any(|m| {
                normalize_eval_token(&m.transcript_form) == normalize_eval_token(variant)
                    && same_eval_term(&m.correct_form, &term.term)
            });
            if !(corrected && removed_variant && edit_policy_match) {
                active_failures += 1;
                if first_active_failure.is_empty() {
                    first_active_failure = format!(
                        "{} -> {} produced '{}' matches={:?}",
                        variant, term.term, result.text, result.matches
                    );
                }
            }
        }
    }
    check(
        total_pass,
        total_fail,
        failures,
        "Dev quality active policy: learned variants correct 100%",
        active_failures == 0,
        &format!("{active_failures}/{total_distortions} failed, first: {first_active_failure}"),
    );

    let mut pollution_failures = 0usize;
    let mut first_pollution_failure = String::new();
    for negative in &suite.negatives {
        let result =
            tier2::correct_with_store(&learned_pool, user_id, &negative.text, &[], &learned_terms);
        for forbidden in &negative.forbidden_terms {
            if !contains_termish(&negative.text, forbidden)
                && contains_termish(&result.text, forbidden)
            {
                pollution_failures += 1;
                if first_pollution_failure.is_empty() {
                    first_pollution_failure = format!(
                        "'{}' injected into '{}' -> '{}'",
                        forbidden, negative.text, result.text
                    );
                }
            }
        }
    }
    check(
        total_pass,
        total_fail,
        failures,
        "Dev quality pollution: active rules do not rewrite negative sentences",
        pollution_failures == 0,
        &format!(
            "{pollution_failures}/{} forbidden checks failed, first: {first_pollution_failure}",
            suite
                .negatives
                .iter()
                .map(|n| n.forbidden_terms.len())
                .sum::<usize>()
        ),
    );
}

fn duplicate_distortions(suite: &DevQualitySuite) -> Vec<String> {
    let mut seen: HashMap<String, String> = HashMap::new();
    let mut conflicts = Vec::new();
    for term in &suite.terms {
        for variant in &term.distortions {
            let norm = tier2_edit_policy::normalize_token(variant);
            if norm.is_empty() {
                continue;
            }
            if let Some(existing_term) = seen.insert(norm.clone(), term.term.clone()) {
                if !same_eval_term(&existing_term, &term.term) {
                    conflicts.push(format!("{variant}: {existing_term} vs {}", term.term));
                }
            }
        }
    }
    conflicts
}

fn trace_points_to_term(result: &ApplyResult, variant: &str, term: &str) -> bool {
    let variant_norm = normalize_eval_token(variant);
    result.traces.iter().any(|trace| {
        normalize_eval_token(&trace.token) == variant_norm && same_eval_term(&trace.candidate, term)
    })
}

fn render_dev_quality_transcript(template: &str, variant: &str) -> String {
    template.replace("{variant}", variant)
}

fn template_context(template: &str) -> (Vec<String>, Vec<String>) {
    let parts: Vec<&str> = template.split_whitespace().collect();
    let Some(idx) = parts.iter().position(|part| part.contains("{variant}")) else {
        return (vec![], vec![]);
    };
    let left = parts[..idx]
        .iter()
        .rev()
        .take(3)
        .map(|part| normalize_eval_token(part))
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let right = parts[idx + 1..]
        .iter()
        .take(3)
        .map(|part| normalize_eval_token(part))
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    (left, right)
}

fn contains_token_norm(text: &str, token: &str) -> bool {
    let needle = normalize_eval_token(token);
    !needle.is_empty()
        && eval_tokens(text)
            .iter()
            .any(|part| normalize_eval_token(part) == needle)
}

fn contains_termish(text: &str, term: &str) -> bool {
    let text_tokens = eval_tokens(text);
    let term_tokens = eval_tokens(term);
    if term_tokens.is_empty() {
        return false;
    }
    if term_tokens.len() == 1 {
        return text_tokens.iter().any(|token| token == &term_tokens[0]);
    }
    text_tokens.windows(term_tokens.len()).any(|window| {
        window
            .iter()
            .zip(term_tokens.iter())
            .all(|(actual, expected)| actual == expected)
    })
}

fn eval_tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        .map(normalize_eval_token)
        .filter(|token| !token.is_empty())
        .collect()
}

fn normalize_eval_token(text: &str) -> String {
    text.trim()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .flat_map(|c| c.to_lowercase())
        .collect()
}

fn same_eval_term(a: &str, b: &str) -> bool {
    normalize_eval_token(a) == normalize_eval_token(b)
}

fn load_dev_quality_suite(path: &PathBuf) -> DevQualitySuite {
    let data = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    serde_json::from_str(&data).expect("bad dev quality JSON")
}

fn default_dev_quality_source() -> String {
    "manual".to_string()
}

fn load_transcripts(path: &PathBuf) -> Vec<TranscriptRow> {
    let data = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    data.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn load_seeds(path: &PathBuf) -> Vec<VocabSeed> {
    let data = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    serde_json::from_str(&data).expect("bad vocab JSON")
}

fn setup_db(seeds: &[VocabSeed]) -> DbPool {
    let tmp = std::env::temp_dir().join(format!(
        "said-eval-{}-{}.db",
        std::process::id(),
        rand_suffix()
    ));
    if tmp.exists() {
        let _ = std::fs::remove_file(&tmp);
    }
    let pool = store::open(&tmp);
    let user_id = "eval-user";
    {
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO local_user (id, email, created_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![user_id, "eval@test.local", said_backend::store::now_ms()],
        )
        .expect("failed to create eval user in setup_db");
    }
    for seed in seeds {
        vocabulary::upsert_for_language_with_context(
            &pool,
            user_id,
            &seed.term,
            3.0,
            &seed.source,
            "hinglish",
            Some(&seed.context),
        );
        vocabulary::update_meaning(&pool, user_id, &seed.term, &seed.meaning);
        vocab_fts::upsert(&pool, user_id, &seed.term, Some(&seed.context));
        let conn = pool.get().unwrap();
        let _ = conn.execute(
            "UPDATE vocabulary SET term_type = ?1 WHERE user_id = ?2 AND term = ?3",
            rusqlite::params![seed.term_type, user_id, seed.term],
        );
    }
    pool
}

fn setup_empty_db() -> DbPool {
    let tmp = std::env::temp_dir().join(format!(
        "said-eval-{}-{}.db",
        std::process::id(),
        rand_suffix()
    ));
    if tmp.exists() {
        let _ = std::fs::remove_file(&tmp);
    }
    store::open(&tmp)
}

fn setup_empty_db_with_user(user_id: &str) -> DbPool {
    let pool = setup_empty_db();
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT OR IGNORE INTO local_user (id, email, created_at) VALUES (?1, ?2, ?3)",
        rusqlite::params![user_id, "eval@test.local", said_backend::store::now_ms()],
    )
    .expect("failed to create eval user");
    // Verify user exists
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM local_user WHERE id = ?1",
            rusqlite::params![user_id],
            |row| row.get(0),
        )
        .unwrap_or(0);
    assert!(count > 0, "eval user not created");
    pool
}

fn rand_suffix() -> u32 {
    use std::sync::atomic::{AtomicU32, Ordering};
    static CTR: AtomicU32 = AtomicU32::new(0);
    CTR.fetch_add(1, Ordering::Relaxed)
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect::<String>() + "..."
    }
}
